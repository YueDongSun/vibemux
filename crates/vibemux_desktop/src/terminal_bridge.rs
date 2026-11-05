//! Per-native-view observations. Core remains authoritative for terminal leases.
use crate::{command_error::CommandError, task_windows::MAX_TASK_WINDOWS};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Mutex};
use uuid::Uuid;
use vibemux_client::{
    daemon_session::DaemonSession, supervisor_model::TerminalView, terminal_binding_view,
    terminal_snapshot_view,
};
use vibemux_types::{ProjectId, RunId, TaskId};
use vibemuxd::{
    control::ControlClient,
    process::DaemonPaths,
    terminal_observer::{TerminalBinding, TerminalLinkRequest, TerminalQuery, TerminalSnapshot},
};

const MAX_OBSERVATIONS: usize = MAX_TASK_WINDOWS + 1;
const PLUGIN_ID: &str = vibemux_client::supervisor_client::TERMINAL_PLUGIN_ID;
const MAX_CANDIDATES: usize = 4;

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum TerminalRequest {
    Inspect {
        #[serde(default)]
        previous_observation_id: Option<Uuid>,
    },
    Link {
        observation_id: Uuid,
        pane_id: String,
        instance_id: String,
    },
    Focus {
        observation_id: Uuid,
    },
    Unlink {
        observation_id: Uuid,
    },
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalOutcome {
    Observed,
    Linked,
    Focused,
    Unlinked,
}
#[derive(Clone, Debug, Serialize)]
pub struct TerminalReply {
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub observation_id: Uuid,
    pub terminal: TerminalView,
    pub outcome: TerminalOutcome,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TerminalActor {
    pub label: String,
    pub generation: Uuid,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TerminalScope {
    pub project_key: String,
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub run_id: RunId,
}
impl TerminalScope {
    fn query(&self) -> TerminalQuery {
        TerminalQuery {
            project_id: self.project_id,
            task_id: self.task_id,
            run_id: self.run_id,
            plugin_id: PLUGIN_ID.into(),
        }
    }
}
#[derive(Clone)]
struct Observation {
    scope: TerminalScope,
    token: Uuid,
    view: TerminalView,
    cleanup_only: bool,
}
struct ViewState {
    generation: Uuid,
    observation: Option<Observation>,
    cleanup: Option<Observation>,
}
pub struct TerminalBridge {
    views: Mutex<HashMap<String, ViewState>>,
}

pub(crate) trait TerminalPort {
    fn inspect(&mut self, query: TerminalQuery) -> Result<TerminalSnapshot, CommandError>;
    fn link(&mut self, request: TerminalLinkRequest) -> Result<TerminalBinding, CommandError>;
    fn focus(&mut self, binding_id: &str) -> Result<(), CommandError>;
    fn unlink(&mut self, binding_id: &str) -> Result<(), CommandError>;
}

impl TerminalBridge {
    pub(crate) fn for_main(generation: Uuid) -> Self {
        Self {
            views: Mutex::new(HashMap::from([(
                "main".into(),
                ViewState {
                    generation,
                    observation: None,
                    cleanup: None,
                },
            )])),
        }
    }
    pub(crate) fn register_window(&self, actor: &TerminalActor) -> Result<(), CommandError> {
        let mut views = self.views.lock().map_err(|_| unavailable())?;
        if let Some(view) = views.get(&actor.label) {
            return if view.generation == actor.generation {
                Ok(())
            } else {
                Err(stale())
            };
        }
        if views.len() >= MAX_OBSERVATIONS {
            return Err(failure("terminal_observation_capacity"));
        }
        views.insert(
            actor.label.clone(),
            ViewState {
                generation: actor.generation,
                observation: None,
                cleanup: None,
            },
        );
        Ok(())
    }
    pub(crate) fn close_window(&self, actor: &TerminalActor) {
        if let Ok(mut views) = self.views.lock()
            && views
                .get(&actor.label)
                .is_some_and(|view| view.generation == actor.generation)
        {
            views.remove(&actor.label);
        }
    }
    pub(crate) fn forget_project(&self, key: &str) {
        if let Ok(mut views) = self.views.lock() {
            for view in views.values_mut() {
                if view
                    .observation
                    .as_ref()
                    .is_some_and(|observation| observation.scope.project_key == key)
                    || view
                        .cleanup
                        .as_ref()
                        .is_some_and(|observation| observation.scope.project_key == key)
                {
                    view.observation = None;
                    view.cleanup = None;
                }
            }
        }
    }

    /// The AppState operation guard serializes effects. This callback is a pure
    /// registration-liveness check; no filesystem or control I/O runs under locks.
    pub(crate) fn execute(
        &self,
        actor: &TerminalActor,
        scope: &TerminalScope,
        request: TerminalRequest,
        port: &mut impl TerminalPort,
        registration_current: impl Fn() -> bool,
    ) -> Result<TerminalReply, CommandError> {
        if !registration_current() {
            return Err(stale());
        }
        {
            let views = self.views.lock().map_err(|_| unavailable())?;
            require_actor(&views, actor)?;
        }
        let query = scope.query();
        let (view, outcome) = match request {
            TerminalRequest::Inspect {
                previous_observation_id,
            } => {
                self.begin_inspection(actor, scope, previous_observation_id)?;
                let snapshot = port.inspect(query.clone())?;
                if snapshot.inventory.panes.len() > MAX_CANDIDATES
                    || snapshot.binding.as_ref().is_some_and(|binding| {
                        binding.query != query
                            || binding.instance_id != snapshot.inventory.instance_id
                            || !snapshot.inventory.panes.contains(&binding.pane)
                    })
                {
                    return Err(failure("terminal_invalid_response"));
                }
                (
                    terminal_snapshot_view(snapshot, PLUGIN_ID),
                    TerminalOutcome::Observed,
                )
            }
            TerminalRequest::Link {
                observation_id,
                pane_id,
                instance_id,
            } => {
                if !valid_id(&pane_id, 20)
                    || !pane_id.bytes().all(|byte| byte.is_ascii_digit())
                    || !valid_id(&instance_id, 128)
                {
                    return Err(failure("invalid_arguments"));
                }
                self.retire_observation(actor, scope, observation_id, false, |observation| {
                    !observation.cleanup_only
                        && observation.view.candidates.iter().any(|candidate| {
                            candidate.plugin_id == PLUGIN_ID
                                && candidate.pane_id == pane_id
                                && candidate.instance_id == instance_id
                        })
                })?;
                // The offer is consumed before submitting a potentially uncertain effect.
                self.require_current(actor, &registration_current)?;
                let binding = port.link(TerminalLinkRequest {
                    query: query.clone(),
                    pane_id: pane_id.clone(),
                    instance_id: instance_id.clone(),
                })?;
                if binding.query != query
                    || binding.pane.pane_id != pane_id
                    || binding.instance_id != instance_id
                {
                    return Err(failure("terminal_invalid_response"));
                }
                (terminal_binding_view(&binding), TerminalOutcome::Linked)
            }
            TerminalRequest::Focus { observation_id } => {
                let observation =
                    self.retire_observation(actor, scope, observation_id, false, |observation| {
                        !observation.cleanup_only && observation.view.binding_id.is_some()
                    })?;
                let id = observation.view.binding_id.as_deref().ok_or_else(stale)?;
                self.require_current(actor, &registration_current)?;
                port.focus(id)?;
                (observation.view, TerminalOutcome::Focused)
            }
            TerminalRequest::Unlink { observation_id } => {
                let observation =
                    self.retire_observation(actor, scope, observation_id, true, |observation| {
                        observation.view.binding_id.is_some()
                    })?;
                let id = observation.view.binding_id.as_deref().ok_or_else(stale)?;
                self.require_current(actor, &registration_current)?;
                port.unlink(id)?;
                (
                    TerminalView {
                        status: "Reviewed terminal observation unlinked".into(),
                        plugin_id: Some(PLUGIN_ID.into()),
                        ..TerminalView::default()
                    },
                    TerminalOutcome::Unlinked,
                )
            }
        };
        if !registration_current() {
            return Err(stale());
        }
        let observation_id = Uuid::new_v4();
        {
            let mut views = self.views.lock().map_err(|_| unavailable())?;
            if !registration_current() {
                return Err(stale());
            }
            let state = views
                .get_mut(&actor.label)
                .filter(|state| state.generation == actor.generation)
                .ok_or_else(stale)?;
            state.observation = Some(Observation {
                scope: scope.clone(),
                token: observation_id,
                view: view.clone(),
                cleanup_only: false,
            });
        }
        Ok(TerminalReply {
            project_id: scope.project_id,
            task_id: scope.task_id,
            run_id: scope.run_id,
            observation_id,
            terminal: view,
            outcome,
        })
    }

    fn require_current(
        &self,
        actor: &TerminalActor,
        registration_current: &impl Fn() -> bool,
    ) -> Result<(), CommandError> {
        let views = self.views.lock().map_err(|_| unavailable())?;
        if !registration_current() {
            return Err(stale());
        }
        require_actor(&views, actor)
    }

    fn begin_inspection(
        &self,
        actor: &TerminalActor,
        scope: &TerminalScope,
        previous: Option<Uuid>,
    ) -> Result<(), CommandError> {
        if previous.is_some_and(|token| token.is_nil()) {
            return Err(failure("invalid_arguments"));
        }
        let mut views = self.views.lock().map_err(|_| unavailable())?;
        let state = views
            .get_mut(&actor.label)
            .filter(|state| state.generation == actor.generation)
            .ok_or_else(stale)?;
        if state
            .observation
            .as_ref()
            .is_some_and(|observation| &observation.scope != scope)
            || state
                .cleanup
                .as_ref()
                .is_some_and(|observation| &observation.scope != scope)
        {
            state.observation = None;
            state.cleanup = None;
        }
        let acknowledged = previous.and_then(|token| {
            state
                .observation
                .as_ref()
                .filter(|observation| observation.token == token && &observation.scope == scope)
                .or_else(|| {
                    state.cleanup.as_ref().filter(|observation| {
                        observation.token == token && &observation.scope == scope
                    })
                })
                .cloned()
        });
        if let Some(mut acknowledged) = acknowledged {
            acknowledged.cleanup_only = true;
            state.cleanup = acknowledged
                .view
                .binding_id
                .is_some()
                .then_some(acknowledged);
        }
        // A failed Find retires old Link/Focus authority. Unknown or omitted
        // acknowledgements cannot promote or erase the reviewed cleanup anchor.
        if let Some(observation) = state.observation.as_mut() {
            observation.cleanup_only = true;
        }
        Ok(())
    }

    fn retire_observation(
        &self,
        actor: &TerminalActor,
        scope: &TerminalScope,
        token: Uuid,
        allow_cleanup: bool,
        allowed: impl Fn(&Observation) -> bool,
    ) -> Result<Observation, CommandError> {
        if token.is_nil() {
            return Err(stale());
        }
        let mut views = self.views.lock().map_err(|_| unavailable())?;
        let view = views
            .get_mut(&actor.label)
            .filter(|view| view.generation == actor.generation)
            .ok_or_else(stale)?;
        let matches = |observation: &&Observation| {
            observation.token == token && &observation.scope == scope && allowed(observation)
        };
        let observation = view
            .observation
            .as_ref()
            .filter(matches)
            .or_else(|| {
                allow_cleanup
                    .then(|| view.cleanup.as_ref().filter(matches))
                    .flatten()
            })
            .cloned()
            .ok_or_else(stale)?;
        view.cleanup = observation.view.binding_id.is_some().then(|| Observation {
            cleanup_only: true,
            ..observation.clone()
        });
        if let Some(current) = view.observation.as_mut() {
            current.cleanup_only = true;
        }
        Ok(observation)
    }
}
fn require_actor(
    views: &HashMap<String, ViewState>,
    actor: &TerminalActor,
) -> Result<(), CommandError> {
    if views
        .get(&actor.label)
        .is_some_and(|view| view.generation == actor.generation)
    {
        Ok(())
    } else {
        Err(stale())
    }
}
fn valid_id(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}
fn stale() -> CommandError {
    failure("terminal_observation_stale")
}
fn unavailable() -> CommandError {
    failure("desktop_terminal_unavailable")
}
fn failure(code: &str) -> CommandError {
    let code = if !code.is_empty()
        && code.len() <= 96
        && code
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        code
    } else {
        "terminal_operation_failed"
    };
    CommandError::new(
        code,
        "Terminal operation unavailable. Find terminals to observe its current state.",
    )
}

pub(crate) struct ControlTerminalPort {
    client: ControlClient,
    runtime: tokio::runtime::Runtime,
}
impl ControlTerminalPort {
    pub(crate) fn connect(paths: &DaemonPaths) -> Result<Self, CommandError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| unavailable())?;
        let (client, _) = DaemonSession::default()
            .connect(paths)
            .map_err(|code| failure(&code))?;
        Ok(Self { client, runtime })
    }
}
impl TerminalPort for ControlTerminalPort {
    fn inspect(&mut self, query: TerminalQuery) -> Result<TerminalSnapshot, CommandError> {
        self.runtime
            .block_on(self.client.terminal_inspect(query))
            .map_err(|error| failure(error.code()))
    }
    fn link(&mut self, request: TerminalLinkRequest) -> Result<TerminalBinding, CommandError> {
        self.runtime
            .block_on(self.client.terminal_link(request))
            .map_err(|error| failure(error.code()))
    }
    fn focus(&mut self, id: &str) -> Result<(), CommandError> {
        self.runtime
            .block_on(self.client.terminal_focus(id))
            .map_err(|error| failure(error.code()))
    }
    fn unlink(&mut self, id: &str) -> Result<(), CommandError> {
        self.runtime
            .block_on(self.client.terminal_unlink(id))
            .map_err(|error| failure(error.code()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    struct FakePort {
        scope: TerminalScope,
        binding: Option<TerminalBinding>,
        calls: Vec<String>,
        unavailable: bool,
        effect_error: bool,
        after_effect: Option<Box<dyn FnOnce()>>,
    }
    impl FakePort {
        fn new(scope: &TerminalScope) -> Self {
            Self {
                scope: scope.clone(),
                binding: None,
                calls: vec![],
                unavailable: false,
                effect_error: false,
                after_effect: None,
            }
        }
        fn snapshot(&self) -> TerminalSnapshot {
            let mut snapshot = TerminalSnapshot {
                inventory: Default::default(),
                binding: self.binding.clone(),
                checked_at_epoch_ms: 1_793_617_200_000,
            };
            snapshot.inventory.instance_id = "fixture_instance".into();
            for id in ["1", "2"] {
                snapshot.inventory.panes.push(Default::default());
                let pane = snapshot.inventory.panes.last_mut().expect("pane");
                pane.pane_id = id.into();
                pane.window_id = "7".into();
                pane.cwd = "C:/PRIVATE_WORKTREE_MARKER".into();
                pane.workspace = "fixture".into();
            }
            snapshot
        }
    }
    impl TerminalPort for FakePort {
        fn inspect(&mut self, query: TerminalQuery) -> Result<TerminalSnapshot, CommandError> {
            self.calls.push("inspect".into());
            if self.unavailable {
                return Err(failure("terminal_plugin_unavailable"));
            }
            assert_eq!(query, self.scope.query());
            Ok(self.snapshot())
        }
        fn link(&mut self, request: TerminalLinkRequest) -> Result<TerminalBinding, CommandError> {
            self.calls.push(format!("link:{}", request.pane_id));
            assert_eq!(request.query, self.scope.query());
            if self.effect_error {
                return Err(failure("control_deadline_exceeded"));
            }
            let pane = self
                .snapshot()
                .inventory
                .panes
                .into_iter()
                .find(|pane| pane.pane_id == request.pane_id)
                .expect("offered pane");
            let binding = TerminalBinding {
                binding_id: Uuid::new_v4().to_string(),
                query: request.query,
                instance_id: request.instance_id,
                pane,
            };
            self.binding = Some(binding.clone());
            Ok(binding)
        }
        fn focus(&mut self, id: &str) -> Result<(), CommandError> {
            self.calls.push(format!("focus:{id}"));
            if self.effect_error
                || self
                    .binding
                    .as_ref()
                    .is_none_or(|binding| binding.binding_id != id)
            {
                return Err(failure("terminal_link_expired"));
            }
            if let Some(after) = self.after_effect.take() {
                after();
            }
            Ok(())
        }
        fn unlink(&mut self, id: &str) -> Result<(), CommandError> {
            self.calls.push(format!("unlink:{id}"));
            if self
                .binding
                .as_ref()
                .is_some_and(|binding| binding.binding_id == id)
            {
                self.binding = None;
            }
            Ok(())
        }
    }
    fn fixture() -> (TerminalBridge, TerminalActor, TerminalScope, FakePort) {
        let actor = TerminalActor {
            label: "main".into(),
            generation: Uuid::new_v4(),
        };
        let bridge = TerminalBridge::for_main(actor.generation);
        let scope = TerminalScope {
            project_key: Uuid::new_v4().to_string(),
            project_id: ProjectId::new(),
            task_id: TaskId::new(),
            run_id: RunId::new(),
        };
        let port = FakePort::new(&scope);
        (bridge, actor, scope, port)
    }
    fn inspect(
        bridge: &TerminalBridge,
        actor: &TerminalActor,
        scope: &TerminalScope,
        port: &mut FakePort,
    ) -> TerminalReply {
        bridge
            .execute(
                actor,
                scope,
                TerminalRequest::Inspect {
                    previous_observation_id: None,
                },
                port,
                || true,
            )
            .expect("inspection")
    }
    fn link(
        bridge: &TerminalBridge,
        actor: &TerminalActor,
        scope: &TerminalScope,
        port: &mut FakePort,
        token: Uuid,
        pane: &str,
    ) -> TerminalReply {
        bridge
            .execute(
                actor,
                scope,
                TerminalRequest::Link {
                    observation_id: token,
                    pane_id: pane.into(),
                    instance_id: "fixture_instance".into(),
                },
                port,
                || true,
            )
            .expect("link")
    }
    #[test]
    fn offers_are_consumed_effects_are_scoped_and_projection_omits_private_cwd() {
        let (bridge, actor, scope, mut port) = fixture();
        let first = inspect(&bridge, &actor, &scope, &mut port);
        assert!(
            !serde_json::to_string(&first)
                .expect("projection")
                .contains("PRIVATE_")
        );
        let linked = link(
            &bridge,
            &actor,
            &scope,
            &mut port,
            first.observation_id,
            "1",
        );
        assert_ne!(first.observation_id, linked.observation_id);
        let calls = port.calls.len();
        assert!(
            bridge
                .execute(
                    &actor,
                    &scope,
                    TerminalRequest::Link {
                        observation_id: first.observation_id,
                        pane_id: "1".into(),
                        instance_id: "fixture_instance".into()
                    },
                    &mut port,
                    || true
                )
                .is_err()
        );
        assert_eq!(port.calls.len(), calls);
        let focused = bridge
            .execute(
                &actor,
                &scope,
                TerminalRequest::Focus {
                    observation_id: linked.observation_id,
                },
                &mut port,
                || true,
            )
            .expect("focus");
        assert_ne!(linked.observation_id, focused.observation_id);
        let unlinked = bridge
            .execute(
                &actor,
                &scope,
                TerminalRequest::Unlink {
                    observation_id: focused.observation_id,
                },
                &mut port,
                || true,
            )
            .expect("unlink");
        assert!(unlinked.terminal.binding_id.is_none());
        assert!(port.binding.is_none());
    }
    #[test]
    fn another_view_replacing_a_lease_never_retargets_stale_controls() {
        let (bridge, main, scope, mut port) = fixture();
        let child = TerminalActor {
            label: "task_fixture".into(),
            generation: Uuid::new_v4(),
        };
        bridge.register_window(&child).expect("child");
        let first = inspect(&bridge, &main, &scope, &mut port);
        let original = link(&bridge, &main, &scope, &mut port, first.observation_id, "1");
        let old_id = original.terminal.binding_id.clone().expect("L1");
        let child_view = inspect(&bridge, &child, &scope, &mut port);
        bridge
            .execute(
                &main,
                &scope,
                TerminalRequest::Unlink {
                    observation_id: original.observation_id,
                },
                &mut port,
                || true,
            )
            .expect("main unlinks L1");
        let offer = inspect(&bridge, &main, &scope, &mut port);
        let replacement = link(&bridge, &main, &scope, &mut port, offer.observation_id, "2");
        let new_id = replacement.terminal.binding_id.clone().expect("L2");
        assert_ne!(old_id, new_id);
        assert_eq!(
            bridge
                .execute(
                    &child,
                    &scope,
                    TerminalRequest::Focus {
                        observation_id: child_view.observation_id
                    },
                    &mut port,
                    || true
                )
                .expect_err("old target gone")
                .code,
            "terminal_link_expired"
        );
        assert_eq!(port.calls.last(), Some(&format!("focus:{old_id}")));
        bridge
            .execute(
                &child,
                &scope,
                TerminalRequest::Unlink {
                    observation_id: child_view.observation_id,
                },
                &mut port,
                || true,
            )
            .expect("cleanup targets only reviewed L1");
        assert_eq!(port.calls.last(), Some(&format!("unlink:{old_id}")));
        assert_eq!(
            port.binding.as_ref().expect("L2 unchanged").binding_id,
            new_id
        );
    }
    #[test]
    fn failed_inspection_preserves_known_unlink_but_uncertain_effect_retires_token() {
        let (bridge, actor, scope, mut port) = fixture();
        let offer = inspect(&bridge, &actor, &scope, &mut port);
        port.effect_error = true;
        assert!(
            bridge
                .execute(
                    &actor,
                    &scope,
                    TerminalRequest::Link {
                        observation_id: offer.observation_id,
                        pane_id: "1".into(),
                        instance_id: "fixture_instance".into()
                    },
                    &mut port,
                    || true
                )
                .is_err()
        );
        let calls = port.calls.len();
        assert!(
            bridge
                .execute(
                    &actor,
                    &scope,
                    TerminalRequest::Link {
                        observation_id: offer.observation_id,
                        pane_id: "1".into(),
                        instance_id: "fixture_instance".into()
                    },
                    &mut port,
                    || true
                )
                .is_err()
        );
        assert_eq!(port.calls.len(), calls);
        port.effect_error = false;
        let fresh = inspect(&bridge, &actor, &scope, &mut port);
        let linked = link(
            &bridge,
            &actor,
            &scope,
            &mut port,
            fresh.observation_id,
            "1",
        );
        port.unavailable = true;
        assert!(
            bridge
                .execute(
                    &actor,
                    &scope,
                    TerminalRequest::Inspect {
                        previous_observation_id: None
                    },
                    &mut port,
                    || true
                )
                .is_err()
        );
        bridge
            .execute(
                &actor,
                &scope,
                TerminalRequest::Unlink {
                    observation_id: linked.observation_id,
                },
                &mut port,
                || true,
            )
            .expect("unlink does not require plugin");
    }

    #[test]
    fn failed_focus_retains_only_reviewed_cleanup_during_plugin_outage() {
        let (bridge, actor, scope, mut port) = fixture();
        let offer = inspect(&bridge, &actor, &scope, &mut port);
        let linked = link(
            &bridge,
            &actor,
            &scope,
            &mut port,
            offer.observation_id,
            "1",
        );
        let reviewed = linked.terminal.binding_id.clone().expect("reviewed lease");
        port.effect_error = true;
        assert!(
            bridge
                .execute(
                    &actor,
                    &scope,
                    TerminalRequest::Focus {
                        observation_id: linked.observation_id
                    },
                    &mut port,
                    || true
                )
                .is_err()
        );
        port.unavailable = true;
        assert!(
            bridge
                .execute(
                    &actor,
                    &scope,
                    TerminalRequest::Inspect {
                        previous_observation_id: None
                    },
                    &mut port,
                    || true
                )
                .is_err()
        );
        let calls = port.calls.len();
        assert!(
            bridge
                .execute(
                    &actor,
                    &scope,
                    TerminalRequest::Focus {
                        observation_id: linked.observation_id
                    },
                    &mut port,
                    || true
                )
                .is_err()
        );
        assert_eq!(port.calls.len(), calls, "effect authority remains retired");
        bridge
            .execute(
                &actor,
                &scope,
                TerminalRequest::Unlink {
                    observation_id: linked.observation_id,
                },
                &mut port,
                || true,
            )
            .expect("cleanup-only unlink works without plugin");
        assert_eq!(port.calls.last(), Some(&format!("unlink:{reviewed}")));
        assert!(port.binding.is_none());
    }
    #[test]
    fn foreign_scope_offer_and_superseded_tokens_reject_before_effect() {
        let (bridge, actor, scope, mut port) = fixture();
        let first = inspect(&bridge, &actor, &scope, &mut port);
        let second = inspect(&bridge, &actor, &scope, &mut port);
        let calls = port.calls.len();
        for (request_scope, token, pane) in [
            (scope.clone(), first.observation_id, "1"),
            (
                TerminalScope {
                    run_id: RunId::new(),
                    ..scope.clone()
                },
                second.observation_id,
                "1",
            ),
            (scope.clone(), second.observation_id, "99"),
        ] {
            assert!(
                bridge
                    .execute(
                        &actor,
                        &request_scope,
                        TerminalRequest::Link {
                            observation_id: token,
                            pane_id: pane.into(),
                            instance_id: "fixture_instance".into()
                        },
                        &mut port,
                        || true
                    )
                    .is_err()
            );
        }
        assert_eq!(port.calls.len(), calls);
    }
    #[test]
    fn closure_or_revocation_during_effect_suppresses_receipt_publication() {
        let (bridge, actor, scope, mut port) = fixture();
        let bridge = Arc::new(bridge);
        let offer = inspect(&bridge, &actor, &scope, &mut port);
        let linked = link(
            &bridge,
            &actor,
            &scope,
            &mut port,
            offer.observation_id,
            "1",
        );
        let close_bridge = Arc::clone(&bridge);
        let close_actor = actor.clone();
        port.after_effect = Some(Box::new(move || close_bridge.close_window(&close_actor)));
        assert!(
            bridge
                .execute(
                    &actor,
                    &scope,
                    TerminalRequest::Focus {
                        observation_id: linked.observation_id
                    },
                    &mut port,
                    || true
                )
                .is_err()
        );
        let successor = TerminalActor {
            generation: Uuid::new_v4(),
            ..actor.clone()
        };
        bridge.register_window(&successor).expect("successor");
        bridge.close_window(&actor);
        let offer = inspect(&bridge, &successor, &scope, &mut port);
        let linked = link(
            &bridge,
            &successor,
            &scope,
            &mut port,
            offer.observation_id,
            "1",
        );
        let active = Arc::new(AtomicBool::new(true));
        let revoked = Arc::clone(&active);
        port.after_effect = Some(Box::new(move || revoked.store(false, Ordering::Release)));
        assert!(
            bridge
                .execute(
                    &successor,
                    &scope,
                    TerminalRequest::Focus {
                        observation_id: linked.observation_id
                    },
                    &mut port,
                    || active.load(Ordering::Acquire)
                )
                .is_err()
        );
    }
    #[test]
    fn bounded_view_cache_forget_and_capacity_never_submit_effects() {
        let (bridge, actor, scope, mut port) = fixture();
        let offer = inspect(&bridge, &actor, &scope, &mut port);
        bridge.forget_project(&scope.project_key);
        assert!(
            bridge
                .execute(
                    &actor,
                    &scope,
                    TerminalRequest::Link {
                        observation_id: offer.observation_id,
                        pane_id: "1".into(),
                        instance_id: "fixture_instance".into()
                    },
                    &mut port,
                    || true
                )
                .is_err()
        );
        for index in 0..MAX_TASK_WINDOWS {
            bridge
                .register_window(&TerminalActor {
                    label: format!("task_{index}"),
                    generation: Uuid::new_v4(),
                })
                .expect("bounded slot");
        }
        assert_eq!(
            bridge
                .register_window(&TerminalActor {
                    label: "overflow".into(),
                    generation: Uuid::new_v4()
                })
                .expect_err("capacity")
                .code,
            "terminal_observation_capacity"
        );
        assert_eq!(port.calls, ["inspect"]);
    }

    #[test]
    fn lost_focus_and_repeated_find_replies_keep_exact_acknowledged_cleanup() {
        let (bridge, actor, scope, mut port) = fixture();
        let offer = inspect(&bridge, &actor, &scope, &mut port);
        let reviewed = link(
            &bridge,
            &actor,
            &scope,
            &mut port,
            offer.observation_id,
            "1",
        );
        let token = reviewed.observation_id;
        let lease = reviewed.terminal.binding_id.expect("L1");
        let lost = bridge
            .execute(
                &actor,
                &scope,
                TerminalRequest::Focus {
                    observation_id: token,
                },
                &mut port,
                || true,
            )
            .expect("Core success, response lost");
        assert_ne!(lost.observation_id, token);
        for _ in 0..3 {
            bridge
                .execute(
                    &actor,
                    &scope,
                    TerminalRequest::Inspect {
                        previous_observation_id: Some(token),
                    },
                    &mut port,
                    || true,
                )
                .expect("Find response also lost");
        }
        port.unavailable = true;
        assert!(
            bridge
                .execute(
                    &actor,
                    &scope,
                    TerminalRequest::Inspect {
                        previous_observation_id: Some(token)
                    },
                    &mut port,
                    || true
                )
                .is_err()
        );
        let calls = port.calls.len();
        assert!(
            bridge
                .execute(
                    &actor,
                    &scope,
                    TerminalRequest::Focus {
                        observation_id: token
                    },
                    &mut port,
                    || true
                )
                .is_err()
        );
        assert_eq!(port.calls.len(), calls, "cleanup token cannot focus");
        bridge
            .execute(
                &actor,
                &scope,
                TerminalRequest::Unlink {
                    observation_id: token,
                },
                &mut port,
                || true,
            )
            .expect("reviewed cleanup survives lost responses");
        assert_eq!(port.calls.last(), Some(&format!("unlink:{lease}")));
        assert!(port.binding.is_none());
    }

    #[test]
    fn acknowledged_successor_supersedes_older_cleanup_authority() {
        let (bridge, actor, scope, mut port) = fixture();
        let offer = inspect(&bridge, &actor, &scope, &mut port);
        let first = link(
            &bridge,
            &actor,
            &scope,
            &mut port,
            offer.observation_id,
            "1",
        );
        let received = bridge
            .execute(
                &actor,
                &scope,
                TerminalRequest::Focus {
                    observation_id: first.observation_id,
                },
                &mut port,
                || true,
            )
            .expect("received successor");
        bridge
            .execute(
                &actor,
                &scope,
                TerminalRequest::Inspect {
                    previous_observation_id: Some(received.observation_id),
                },
                &mut port,
                || true,
            )
            .expect("acknowledge received successor");
        let calls = port.calls.len();
        assert!(
            bridge
                .execute(
                    &actor,
                    &scope,
                    TerminalRequest::Unlink {
                        observation_id: first.observation_id
                    },
                    &mut port,
                    || true
                )
                .is_err()
        );
        assert_eq!(port.calls.len(), calls);
        bridge
            .execute(
                &actor,
                &scope,
                TerminalRequest::Unlink {
                    observation_id: received.observation_id,
                },
                &mut port,
                || true,
            )
            .expect("only acknowledged cleanup remains");
    }

    #[test]
    fn missing_unknown_or_foreign_ack_cannot_erase_or_promote_cleanup() {
        let (bridge, actor, scope, mut port) = fixture();
        let offer = inspect(&bridge, &actor, &scope, &mut port);
        let first = link(
            &bridge,
            &actor,
            &scope,
            &mut port,
            offer.observation_id,
            "1",
        );
        bridge
            .execute(
                &actor,
                &scope,
                TerminalRequest::Focus {
                    observation_id: first.observation_id,
                },
                &mut port,
                || true,
            )
            .expect("lost successor");
        let other = TerminalActor {
            label: "task_other".into(),
            generation: Uuid::new_v4(),
        };
        bridge.register_window(&other).expect("other view");
        let foreign = inspect(&bridge, &other, &scope, &mut port);
        for previous in [None, Some(Uuid::new_v4()), Some(foreign.observation_id)] {
            bridge
                .execute(
                    &actor,
                    &scope,
                    TerminalRequest::Inspect {
                        previous_observation_id: previous,
                    },
                    &mut port,
                    || true,
                )
                .expect("read-only Find still allowed");
        }
        let calls = port.calls.len();
        assert!(
            bridge
                .execute(
                    &actor,
                    &scope,
                    TerminalRequest::Unlink {
                        observation_id: foreign.observation_id
                    },
                    &mut port,
                    || true
                )
                .is_err()
        );
        assert_eq!(
            port.calls.len(),
            calls,
            "another view's token grants no cleanup"
        );
        bridge
            .execute(
                &actor,
                &scope,
                TerminalRequest::Unlink {
                    observation_id: first.observation_id,
                },
                &mut port,
                || true,
            )
            .expect("known anchor not erased");
    }

    #[test]
    fn lost_replacement_result_cleanup_never_substitutes_the_new_lease() {
        let (bridge, actor, scope, mut port) = fixture();
        let offer = inspect(&bridge, &actor, &scope, &mut port);
        let old = link(
            &bridge,
            &actor,
            &scope,
            &mut port,
            offer.observation_id,
            "1",
        );
        let reviewed = bridge
            .execute(
                &actor,
                &scope,
                TerminalRequest::Inspect {
                    previous_observation_id: Some(old.observation_id),
                },
                &mut port,
                || true,
            )
            .expect("reviewed L1 and candidates");
        let old_id = reviewed.terminal.binding_id.clone().expect("L1");
        let lost = link(
            &bridge,
            &actor,
            &scope,
            &mut port,
            reviewed.observation_id,
            "2",
        );
        let new_id = lost.terminal.binding_id.expect("unacknowledged L2");
        port.unavailable = true;
        assert!(
            bridge
                .execute(
                    &actor,
                    &scope,
                    TerminalRequest::Inspect {
                        previous_observation_id: Some(reviewed.observation_id)
                    },
                    &mut port,
                    || true
                )
                .is_err()
        );
        bridge
            .execute(
                &actor,
                &scope,
                TerminalRequest::Unlink {
                    observation_id: reviewed.observation_id,
                },
                &mut port,
                || true,
            )
            .expect("cleanup only L1");
        assert_eq!(port.calls.last(), Some(&format!("unlink:{old_id}")));
        assert_eq!(
            port.binding.as_ref().expect("L2 untouched").binding_id,
            new_id
        );
    }

    #[test]
    fn lost_unlink_ack_allows_idempotent_cleanup_of_only_the_reviewed_lease() {
        let (bridge, actor, scope, mut port) = fixture();
        let offer = inspect(&bridge, &actor, &scope, &mut port);
        let reviewed = link(
            &bridge,
            &actor,
            &scope,
            &mut port,
            offer.observation_id,
            "1",
        );
        let old_id = reviewed.terminal.binding_id.expect("L1");
        bridge
            .execute(
                &actor,
                &scope,
                TerminalRequest::Unlink {
                    observation_id: reviewed.observation_id,
                },
                &mut port,
                || true,
            )
            .expect("unlink response lost");
        let replacement = port
            .link(TerminalLinkRequest {
                query: scope.query(),
                instance_id: "fixture_instance".into(),
                pane_id: "2".into(),
            })
            .expect("other view creates L2");
        bridge
            .execute(
                &actor,
                &scope,
                TerminalRequest::Unlink {
                    observation_id: reviewed.observation_id,
                },
                &mut port,
                || true,
            )
            .expect("explicit cleanup retry");
        assert_eq!(port.calls.last(), Some(&format!("unlink:{old_id}")));
        assert_eq!(
            port.binding
                .as_ref()
                .expect("other link preserved")
                .binding_id,
            replacement.binding_id
        );
    }
}
