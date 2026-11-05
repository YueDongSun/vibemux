//! Fresh Core workspace reads followed by narrowly scoped native effects.
use crate::{
    command_error::CommandError,
    link_contract::{LinkOutcome, OpenLinkReply, WorktreeAction},
    link_effects::LinkEffects,
    workspace_path_pin::WorkspacePathPin,
};
use std::path::Path;
use vibemux_client::daemon_session::DaemonSession;
use vibemux_types::{ProjectId, RunId, TaskId};
use vibemuxd::{
    control::{ControlClient, WorkspaceReadMode, WorkspaceReadQuery, WorkspaceReadView},
    process::DaemonPaths,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkspaceScope {
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub run_id: RunId,
}

pub(crate) trait WorkspacePort {
    fn read(&mut self, query: WorkspaceReadQuery) -> Result<WorkspaceReadView, CommandError>;
}

pub(crate) struct ControlWorkspacePort {
    client: ControlClient,
    runtime: tokio::runtime::Runtime,
}
impl ControlWorkspacePort {
    pub(crate) fn connect(paths: &DaemonPaths) -> Result<Self, CommandError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| failure("workspace_read_failed"))?;
        let (client, _) = DaemonSession::default()
            .connect(paths)
            .map_err(|code| failure(&code))?;
        Ok(Self { client, runtime })
    }
}
impl WorkspacePort for ControlWorkspacePort {
    fn read(&mut self, query: WorkspaceReadQuery) -> Result<WorkspaceReadView, CommandError> {
        self.runtime
            .block_on(self.client.workspace_read(query))
            .map_err(|error| failure(error.code()))
    }
}

pub(crate) fn worktree_action(
    root: &Path,
    scope: &WorkspaceScope,
    action: WorktreeAction,
    port: &mut impl WorkspacePort,
    effects: &dyn LinkEffects,
    current: impl Fn() -> bool,
) -> Result<OpenLinkReply, CommandError> {
    require_current(&current)?;
    let query = WorkspaceReadQuery {
        project_id: scope.project_id,
        task_id: scope.task_id,
        run_id: scope.run_id,
        mode: if action == WorktreeAction::CopyBranch {
            WorkspaceReadMode::Metadata
        } else {
            WorkspaceReadMode::Directory
        },
    };
    let first = port.read(query.clone())?;
    first.validate_for(&query).map_err(failure)?;
    require_current(&current)?;
    let outcome = if action == WorktreeAction::CopyBranch {
        let branch = first
            .branch
            .as_deref()
            .ok_or_else(|| failure("workspace_branch_unavailable"))?;
        effects
            .copy_text(branch)
            .map_err(|error| failure(&error.code))?;
        LinkOutcome::BranchCopied
    } else {
        let directory = first
            .directory
            .as_ref()
            .ok_or_else(|| failure("workspace_directory_unavailable"))?;
        let pin = WorkspacePathPin::acquire(root, Path::new(&directory.path))?;
        require_current(&current)?;
        // A complete second Core inspection runs while native directory handles
        // remain held. Logical keys do not substitute for OS object identity.
        let second = port.read(query.clone())?;
        second.validate_for(&query).map_err(failure)?;
        if second != first {
            return Err(failure("workspace_authority_changed"));
        }
        pin.recheck()?;
        require_current(&current)?;
        match action {
            WorktreeAction::Open => {
                effects
                    .open_directory(Path::new(&directory.path))
                    .map_err(|error| failure(&error.code))?;
                LinkOutcome::OpenRequested
            }
            WorktreeAction::CopyPath => {
                effects
                    .copy_text(&directory.path)
                    .map_err(|error| failure(&error.code))?;
                LinkOutcome::PathCopied
            }
            WorktreeAction::CopyBranch => return Err(failure("invalid_arguments")),
        }
    };
    require_current(&current)?;
    Ok(OpenLinkReply::Worktree {
        project_id: scope.project_id,
        task_id: scope.task_id,
        run_id: scope.run_id,
        action,
        outcome,
    })
}

fn require_current(current: &impl Fn() -> bool) -> Result<(), CommandError> {
    if current() {
        Ok(())
    } else {
        Err(failure("link_caller_unavailable"))
    }
}

pub(crate) fn failure(code: &str) -> CommandError {
    let code = if !code.is_empty()
        && code.len() <= 96
        && code
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        code
    } else {
        "workspace_access_failed"
    };
    CommandError::new(
        code,
        "The native presentation action could not be confirmed.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::VecDeque,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        },
    };
    use vibemuxd::control::WorkspaceDirectory;

    #[derive(Default)]
    struct Effects(Mutex<Vec<String>>);
    impl LinkEffects for Effects {
        fn open_https(&self, _: &url::Url) -> Result<(), CommandError> {
            self.0.lock().expect("effects").push("https".into());
            Ok(())
        }
        fn open_directory(&self, path: &Path) -> Result<(), CommandError> {
            self.0
                .lock()
                .expect("effects")
                .push(format!("open:{}", path.display()));
            Ok(())
        }
        fn copy_text(&self, text: &str) -> Result<(), CommandError> {
            self.0.lock().expect("effects").push(format!("copy:{text}"));
            Ok(())
        }
    }
    struct Port {
        views: VecDeque<WorkspaceReadView>,
        calls: Vec<WorkspaceReadQuery>,
        after: Option<Box<dyn FnOnce()>>,
    }
    impl WorkspacePort for Port {
        fn read(&mut self, query: WorkspaceReadQuery) -> Result<WorkspaceReadView, CommandError> {
            self.calls.push(query);
            if let Some(after) = self.after.take() {
                after();
            }
            self.views
                .pop_front()
                .ok_or_else(|| failure("workspace_read_failed"))
        }
    }
    fn scope() -> WorkspaceScope {
        WorkspaceScope {
            project_id: ProjectId::new(),
            task_id: TaskId::new(),
            run_id: RunId::new(),
        }
    }
    fn view(scope: &WorkspaceScope, path: Option<String>) -> WorkspaceReadView {
        WorkspaceReadView {
            project_id: scope.project_id,
            task_id: scope.task_id,
            run_id: scope.run_id,
            branch: Some("recorded/complete_branch".into()),
            directory: path.map(|path| WorkspaceDirectory {
                path,
                resource_key: "a".repeat(64),
            }),
        }
    }
    fn port(views: Vec<WorkspaceReadView>) -> Port {
        Port {
            views: views.into(),
            calls: vec![],
            after: None,
        }
    }

    #[test]
    fn branch_copy_uses_one_metadata_read_without_directory_existence_or_reply_payload() {
        let scope = scope();
        let expected = view(&scope, None);
        let mut port = port(vec![expected.clone()]);
        let effects = Effects::default();
        let reply = worktree_action(
            Path::new("unused"),
            &scope,
            WorktreeAction::CopyBranch,
            &mut port,
            &effects,
            || true,
        )
        .expect("branch");
        assert_eq!(port.calls.len(), 1);
        assert_eq!(port.calls[0].mode, WorkspaceReadMode::Metadata);
        assert_eq!(
            *effects.0.lock().expect("effects"),
            vec!["copy:recorded/complete_branch"]
        );
        let encoded = serde_json::to_string(&reply).expect("reply");
        assert!(!encoded.contains("complete_branch"));
        assert!(!encoded.contains("resource_key"));
    }

    #[test]
    fn directory_effects_require_two_matching_fresh_reads_and_hold_real_pins() {
        let temp = tempfile::tempdir().expect("owned root");
        let root = std::fs::canonicalize(temp.path()).expect("root");
        let path = root.join("run");
        std::fs::create_dir(&path).expect("directory");
        for action in [WorktreeAction::Open, WorktreeAction::CopyPath] {
            let scope = scope();
            let snapshot = view(&scope, Some(path.to_str().expect("UTF-8").into()));
            let mut port = port(vec![snapshot.clone(), snapshot]);
            let effects = Effects::default();
            worktree_action(&root, &scope, action, &mut port, &effects, || true).expect("effect");
            assert_eq!(port.calls.len(), 2);
            assert!(
                port.calls
                    .iter()
                    .all(|query| query.mode == WorkspaceReadMode::Directory)
            );
            assert_eq!(effects.0.lock().expect("effects").len(), 1);
        }
    }

    #[test]
    fn foreign_changed_or_unavailable_reads_cannot_cause_a_native_effect() {
        let temp = tempfile::tempdir().expect("owned root");
        let root = std::fs::canonicalize(temp.path()).expect("root");
        let path = root.join("run");
        std::fs::create_dir(&path).expect("directory");
        for fault in ["foreign", "changed", "unavailable"] {
            let scope = scope();
            let first = view(&scope, Some(path.to_str().expect("UTF-8").into()));
            let mut second = first.clone();
            if fault == "foreign" {
                second.run_id = RunId::new();
            } else {
                second.directory.as_mut().expect("directory").resource_key = "b".repeat(64);
            }
            let views = if fault == "unavailable" {
                vec![first]
            } else {
                vec![first, second]
            };
            let mut port = port(views);
            let effects = Effects::default();
            assert!(
                worktree_action(
                    &root,
                    &scope,
                    WorktreeAction::Open,
                    &mut port,
                    &effects,
                    || true
                )
                .is_err()
            );
            assert!(effects.0.lock().expect("effects").is_empty());
        }
    }

    #[test]
    fn caller_revocation_after_a_read_prevents_handoff() {
        let scope = scope();
        let alive = Arc::new(AtomicBool::new(true));
        let revoke = Arc::clone(&alive);
        let mut port = port(vec![view(&scope, None)]);
        port.after = Some(Box::new(move || {
            revoke.store(false, Ordering::Release);
        }));
        let effects = Effects::default();
        assert!(
            worktree_action(
                Path::new("unused"),
                &scope,
                WorktreeAction::CopyBranch,
                &mut port,
                &effects,
                || alive.load(Ordering::Acquire)
            )
            .is_err()
        );
        assert!(effects.0.lock().expect("effects").is_empty());
    }
}
