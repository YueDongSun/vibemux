//! One app-owned diagnostic worker, inert operation replay and joined shutdown.
use crate::{
    command_error::CommandError,
    probe_contract::{
        PROBE_VIEW_VERSION, ProbeJobView, ProbePhase, ProbeRequest, ProbeResultView, ProbeSnapshot,
        failure,
    },
    project_registry::RegisteredProject,
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    thread::JoinHandle,
    time::{Duration, Instant},
};
use tokio::sync::watch;
use uuid::Uuid;

const MAX_OPERATIONS: usize = 64;
const MAX_PUBLIC_REPORT_BYTES: usize = 16 * 1024;
const SHUTDOWN_WAIT: Duration = Duration::from_secs(35);
const SHUTDOWN_POLL: Duration = Duration::from_millis(10);

/// Trusted native composition only; no renderer selects a runner or its outcome.
pub trait ProbeRunner: Send + Sync + std::any::Any {
    fn supported(&self) -> bool {
        true
    }
    fn run(&self, cancel: watch::Receiver<bool>) -> ProbeOutcome;
}
pub struct ProbeOutcome {
    pub report: Option<Value>,
    pub error_code: Option<String>,
    pub quiescent: bool,
}
pub struct SharedProbeRunner;
impl ProbeRunner for SharedProbeRunner {
    fn supported(&self) -> bool {
        cfg!(windows)
    }
    fn run(&self, cancel: watch::Receiver<bool>) -> ProbeOutcome {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(_) => {
                return ProbeOutcome {
                    report: None,
                    error_code: Some("local_probe_runtime_unavailable".into()),
                    quiescent: true,
                };
            }
        };
        let completion = runtime.block_on(vibemuxd::local_probe::run(cancel));
        ProbeOutcome {
            report: completion.report().cloned(),
            error_code: completion.error_code().map(str::to_owned),
            quiescent: completion.is_quiescent(),
        }
    }
}

struct EntryState {
    view: ProbeJobView,
    report: Option<Value>,
}
struct Entry {
    project: Arc<RegisteredProject>,
    operation_id: Uuid,
    cancel: watch::Sender<bool>,
    state: Mutex<EntryState>,
}
#[derive(Default)]
struct RegistryData {
    entries: BTreeMap<Uuid, Arc<Entry>>,
    latest: BTreeMap<String, Uuid>,
    completed: BTreeMap<String, Uuid>,
    workers: Vec<JoinHandle<()>>,
    active: Option<Uuid>,
    stopping: bool,
    join_deadline: Option<Instant>,
}
pub struct ProbeRegistry {
    data: Arc<Mutex<RegistryData>>,
    runner: Arc<dyn ProbeRunner>,
}
impl ProbeRegistry {
    #[cfg(test)]
    pub(crate) fn uses_runner<T: ProbeRunner>(&self) -> bool {
        self.runner.as_ref().type_id() == std::any::TypeId::of::<T>()
    }

    pub fn new(runner: Arc<dyn ProbeRunner>) -> Self {
        Self {
            data: Arc::new(Mutex::new(RegistryData::default())),
            runner,
        }
    }

    pub fn request(
        &self,
        project: Arc<RegisteredProject>,
        request: ProbeRequest,
    ) -> Result<ProbeSnapshot, CommandError> {
        request.validate()?;
        if matches!(request, ProbeRequest::Start { .. }) {
            project.resolve()?;
        }
        let mut data = self
            .data
            .lock()
            .map_err(|_| failure("probe_state_unavailable"))?;
        let selected =
            match request {
                ProbeRequest::Start { operation_id } => {
                    if let Some(entry) = data.entries.get(&operation_id) {
                        require_scope(entry, &project)?;
                    } else {
                        if data.stopping {
                            return Err(failure("probe_stopping"));
                        }
                        if !self.runner.supported() {
                            return Err(failure("probe_unsupported"));
                        }
                        if data.active.is_some() {
                            return Err(failure("probe_busy"));
                        }
                        if data.entries.len() >= MAX_OPERATIONS {
                            return Err(failure("probe_history_full"));
                        }
                        let (cancel, receiver) = watch::channel(false);
                        let entry = Arc::new(Entry {
                            project: Arc::clone(&project),
                            operation_id,
                            cancel,
                            state: Mutex::new(EntryState {
                                view: ProbeJobView {
                                    operation_id,
                                    phase: ProbePhase::Running,
                                    error_code: None,
                                },
                                report: None,
                            }),
                        });
                        data.entries.insert(operation_id, Arc::clone(&entry));
                        data.latest.insert(project.key.clone(), operation_id);
                        data.active = Some(operation_id);
                        let shared = Arc::clone(&self.data);
                        let runner = Arc::clone(&self.runner);
                        let launched =
                            std::thread::Builder::new()
                                .name("desktop_local_probe".into())
                                .spawn(move || {
                                    let outcome = std::panic::catch_unwind(
                                        std::panic::AssertUnwindSafe(|| runner.run(receiver)),
                                    )
                                    .unwrap_or_else(|_| ProbeOutcome {
                                        report: None,
                                        error_code: Some("local_probe_worker_failed".into()),
                                        quiescent: false,
                                    });
                                    finish(&shared, &entry, outcome);
                                });
                        match launched {
                            Ok(worker) => data.workers.push(worker),
                            Err(_) => {
                                data.active = None;
                                if let Some(entry) = data.entries.get(&operation_id)
                                    && let Ok(mut state) = entry.state.lock()
                                {
                                    state.view.phase = ProbePhase::Failed;
                                    state.view.error_code =
                                        Some("local_probe_worker_unavailable".into());
                                }
                            }
                        }
                    }
                    Some(operation_id)
                }
                ProbeRequest::Status { operation_id } => operation_id.or_else(|| {
                    data.latest.get(&project.key).copied().filter(|id| {
                        data.entries
                            .get(id)
                            .is_some_and(|entry| Arc::ptr_eq(&entry.project, &project))
                    })
                }),
                ProbeRequest::Cancel { operation_id } => {
                    let entry = data
                        .entries
                        .get(&operation_id)
                        .ok_or_else(|| failure("probe_operation_not_found"))?;
                    require_scope(entry, &project)?;
                    let running = entry
                        .state
                        .lock()
                        .map_err(|_| failure("probe_state_unavailable"))?
                        .view
                        .phase
                        == ProbePhase::Running;
                    if running && entry.cancel.send(true).is_err() {
                        return Err(failure("probe_cancel_unconfirmed"));
                    }
                    Some(operation_id)
                }
            };
        snapshot(&data, &project, selected, self.runner.supported())
    }

    pub fn has_unsettled(&self, key: &str) -> bool {
        self.data
            .lock()
            .map(|data| {
                data.active
                    .and_then(|id| data.entries.get(&id))
                    .is_some_and(|entry| entry.project.key == key)
            })
            .unwrap_or(true)
    }
    pub fn begin_shutdown(&self) {
        if let Ok(mut data) = self.data.lock() {
            data.stopping = true;
            for entry in data.entries.values() {
                let _ = entry.cancel.send(true);
            }
        }
    }
    pub fn unresolved(&self) -> usize {
        self.data
            .lock()
            .map(|data| usize::from(data.active.is_some()).max(data.workers.len()))
            .unwrap_or(1)
    }
    pub fn shutdown(&self) -> usize {
        self.shutdown_with_wait(SHUTDOWN_WAIT)
    }
    fn shutdown_with_wait(&self, wait: Duration) -> usize {
        self.begin_shutdown();
        let deadline = match self.data.lock() {
            Ok(mut data) => *data
                .join_deadline
                .get_or_insert_with(|| Instant::now() + wait),
            Err(_) => return 1,
        };
        loop {
            let (finished, waiting) = match self.data.lock() {
                Ok(mut data) => {
                    let mut finished = Vec::new();
                    let mut waiting = Vec::new();
                    for worker in data.workers.drain(..) {
                        if worker.is_finished() {
                            finished.push(worker);
                        } else {
                            waiting.push(worker);
                        }
                    }
                    data.workers = waiting;
                    (finished, data.workers.len())
                }
                Err(_) => return 1,
            };
            for worker in finished {
                let _ = worker.join();
            }
            if waiting == 0 || Instant::now() >= deadline {
                return self
                    .data
                    .lock()
                    .map(|data| usize::from(data.active.is_some()).max(data.workers.len()))
                    .unwrap_or(1);
            }
            std::thread::sleep(SHUTDOWN_POLL);
        }
    }
}
impl Drop for ProbeRegistry {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn require_scope(entry: &Entry, project: &Arc<RegisteredProject>) -> Result<(), CommandError> {
    if !Arc::ptr_eq(&entry.project, project) {
        return Err(failure("probe_operation_conflict"));
    }
    Ok(())
}
fn snapshot(
    data: &RegistryData,
    project: &Arc<RegisteredProject>,
    selected: Option<Uuid>,
    supported: bool,
) -> Result<ProbeSnapshot, CommandError> {
    let mut result = None;
    let job = if let Some(id) = selected {
        let entry = data
            .entries
            .get(&id)
            .ok_or_else(|| failure("probe_operation_not_found"))?;
        require_scope(entry, project)?;
        let state = entry
            .state
            .lock()
            .map_err(|_| failure("probe_state_unavailable"))?;
        if let Some(report) = &state.report {
            result = Some(ProbeResultView {
                operation_id: id,
                report: report.clone(),
            });
        }
        Some(state.view.clone())
    } else {
        None
    };
    if result.is_none()
        && let Some(id) = data.completed.get(&project.key)
        && let Some(entry) = data.entries.get(id)
        && Arc::ptr_eq(&entry.project, project)
        && let Ok(state) = entry.state.lock()
        && let Some(report) = &state.report
    {
        result = Some(ProbeResultView {
            operation_id: *id,
            report: report.clone(),
        });
    }
    let blocking = if data.stopping {
        Some("probe_stopping")
    } else if !supported {
        Some("probe_unsupported")
    } else if data.active.is_some() {
        Some("probe_busy")
    } else if data.entries.len() >= MAX_OPERATIONS {
        Some("probe_history_full")
    } else {
        None
    };
    Ok(ProbeSnapshot {
        schema_version: PROBE_VIEW_VERSION,
        project_key: project.key.clone(),
        job,
        result,
        can_start: blocking.is_none(),
        blocking_code: blocking.map(str::to_owned),
    })
}
fn finish(data: &Mutex<RegistryData>, entry: &Entry, mut outcome: ProbeOutcome) {
    if outcome.report.as_ref().is_some_and(|report| {
        serde_json::to_vec(report).map_or(true, |bytes| bytes.len() > MAX_PUBLIC_REPORT_BYTES)
    }) {
        outcome.report = None;
        outcome.error_code = Some("local_probe_report_invalid".into());
    }
    let phase = if !outcome.quiescent {
        ProbePhase::Unconfirmed
    } else if outcome.error_code.as_deref() == Some("local_probe_cancelled") {
        ProbePhase::Cancelled
    } else if outcome.error_code.is_some() || outcome.report.is_none() {
        ProbePhase::Failed
    } else {
        ProbePhase::Completed
    };
    let code = if phase == ProbePhase::Unconfirmed {
        Some("local_probe_cleanup_unconfirmed".into())
    } else {
        outcome
            .error_code
            .map(|code| bounded_code(&code))
            .or_else(|| (phase == ProbePhase::Failed).then(|| "local_probe_report_invalid".into()))
    };
    if let Ok(mut data) = data.lock()
        && let Ok(mut state) = entry.state.lock()
    {
        state.view.phase = phase;
        state.view.error_code = code;
        state.report = (phase == ProbePhase::Completed)
            .then_some(outcome.report)
            .flatten();
        if phase == ProbePhase::Completed {
            data.completed
                .insert(entry.project.key.clone(), entry.operation_id);
        }
        if outcome.quiescent && data.active == Some(entry.operation_id) {
            data.active = None;
        }
    }
}
fn bounded_code(code: &str) -> String {
    if code.len() <= 96
        && !code.is_empty()
        && code
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
    {
        code.into()
    } else {
        "local_probe_failed".into()
    }
}

#[cfg(test)]
mod tests;
