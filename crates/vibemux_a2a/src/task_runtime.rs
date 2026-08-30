//! Bounded, owned ephemeral execution shared by model peers and the daemon.
//! No canonical-state access exists here. Executors receive explicit capabilities.
use crate::task_contract::*;
use async_trait::async_trait;
use futures::stream;
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::{broadcast, mpsc, oneshot, watch},
    task::{Id, JoinError, JoinHandle, JoinSet},
    time::timeout,
};

const REQUEST_CAPACITY: usize = 32;
const EVENT_CAPACITY: usize = 16;
const MAX_RUNTIME_TASKS: usize = 64;
const MAX_CANCEL_WAITERS: usize = 8;
const MAX_SUBSCRIPTIONS: usize = 32;
const RUNTIME_SHUTDOWN: Duration = Duration::from_secs(10);
const MAX_EXACT_INTEGER: i64 = 9_007_199_254_740_991;
const MAX_EXACT_INTEGER_FLOAT: f64 = 9_007_199_254_740_991.0;

pub enum TaskExecution {
    Completed(Vec<TaskArtifact>),
    Canceled,
}
#[async_trait]
pub trait TaskExecutor: Send + Sync + 'static {
    async fn execute(
        &self,
        subject: &str,
        request: TaskRequest,
        cancellation: watch::Receiver<bool>,
    ) -> Result<TaskExecution, TaskGatewayError>;
}
pub struct TaskRuntime {
    backend: Arc<RuntimeBackend>,
    stop: watch::Sender<bool>,
    owner: Option<JoinHandle<Result<(), TaskGatewayError>>>,
}
#[derive(Clone)]
pub struct RuntimeBackend {
    sender: mpsc::Sender<Command>,
}
type Reply<T> = oneshot::Sender<Result<T, TaskGatewayError>>;
type JobResult = (String, Result<TaskExecution, TaskGatewayError>);
type JoinedJob = Result<(Id, JobResult), JoinError>;
enum Command {
    Send {
        subject: String,
        request: TaskRequest,
        reply: Reply<TaskSnapshot>,
    },
    Get {
        subject: String,
        id: String,
        reply: Reply<TaskSnapshot>,
    },
    Cancel {
        subject: String,
        id: String,
        reply: Reply<TaskSnapshot>,
    },
    Subscribe {
        subject: String,
        id: String,
        reply: Reply<TaskEventStream>,
    },
    List {
        subject: String,
        request: TaskListRequest,
        reply: Reply<TaskList>,
    },
}
struct Entry {
    subject: String,
    key: String,
    input: Vec<u8>,
    snapshot: TaskSnapshot,
    events: broadcast::Sender<TaskSnapshot>,
    cancel: watch::Sender<bool>,
    cancel_waiters: Vec<Reply<TaskSnapshot>>,
}
struct Owner {
    entries: BTreeMap<String, Entry>,
    jobs: JoinSet<JobResult>,
    job_ids: HashMap<Id, String>,
    failure: Option<TaskGatewayError>,
}
impl Owner {
    fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            jobs: JoinSet::new(),
            job_ids: HashMap::new(),
            failure: None,
        }
    }
    fn joined(&mut self, result: JoinedJob, aborted: Option<TaskGatewayError>) {
        match result {
            Ok((job_id, (id, outcome))) => {
                if self.job_ids.remove(&job_id).as_deref() != Some(id.as_str()) {
                    self.failure.get_or_insert(TaskGatewayError::Internal);
                }
                finish(&mut self.entries, &id, outcome);
            }
            Err(error) => {
                let failure = aborted.unwrap_or(TaskGatewayError::Internal);
                if let Some(id) = self.job_ids.remove(&error.id()) {
                    finish(&mut self.entries, &id, Err(failure.clone()));
                }
                self.failure.get_or_insert(failure);
            }
        }
    }
    fn cancel_all(&self) {
        for entry in self.entries.values() {
            if !entry.snapshot.state.is_terminal() {
                entry.cancel.send_replace(true);
            }
        }
    }
    async fn drain(&mut self) -> Result<(), TaskGatewayError> {
        self.cancel_all();
        let drained = timeout(RUNTIME_SHUTDOWN, async {
            while let Some(result) = self.jobs.join_next_with_id().await {
                self.joined(result, None);
            }
        })
        .await;
        if drained.is_err() {
            self.jobs.abort_all();
            while let Some(result) = self.jobs.join_next_with_id().await {
                self.joined(result, Some(TaskGatewayError::Deadline));
            }
            let pending = self
                .entries
                .iter()
                .filter(|(_, entry)| !entry.snapshot.state.is_terminal())
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            for id in pending {
                finish(&mut self.entries, &id, Err(TaskGatewayError::Deadline));
            }
            return Err(TaskGatewayError::Deadline);
        }
        self.failure.clone().map_or(Ok(()), Err)
    }
}
impl TaskRuntime {
    pub fn start(
        executor: Arc<dyn TaskExecutor>,
        max_parallel: usize,
    ) -> Result<Self, TaskGatewayError> {
        if !(1..=8).contains(&max_parallel) {
            return Err(TaskGatewayError::InvalidRequest);
        }
        let runtime =
            tokio::runtime::Handle::try_current().map_err(|_| TaskGatewayError::Server)?;
        let (sender, receiver) = mpsc::channel(REQUEST_CAPACITY);
        let (stop, stop_receiver) = watch::channel(false);
        let owner = runtime.spawn(run_owner(executor, max_parallel, receiver, stop_receiver));
        Ok(Self {
            backend: Arc::new(RuntimeBackend { sender }),
            stop,
            owner: Some(owner),
        })
    }
    pub fn backend(&self) -> Arc<RuntimeBackend> {
        self.backend.clone()
    }
    pub async fn shutdown(mut self) -> Result<(), TaskGatewayError> {
        self.stop.send_replace(true);
        // Retain the handle across await so cancellation never loses its stop signal.
        let owner = self.owner.as_mut().ok_or(TaskGatewayError::Server)?;
        let result = owner.await.map_err(|_| TaskGatewayError::Server)?;
        self.owner.take();
        result
    }
}
impl Drop for TaskRuntime {
    fn drop(&mut self) {
        // Emergency cancellation only. Explicit shutdown is the joined receipt;
        // the owner continues its bounded drain if this handle is dropped early.
        self.stop.send_replace(true);
    }
}
impl RuntimeBackend {
    async fn request<T>(
        &self,
        make: impl FnOnce(Reply<T>) -> Command,
    ) -> Result<T, TaskGatewayError> {
        let (sender, receiver) = oneshot::channel();
        self.sender
            .try_send(make(sender))
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => TaskGatewayError::Busy,
                mpsc::error::TrySendError::Closed(_) => TaskGatewayError::Server,
            })?;
        timeout(TASK_CALL_DEADLINE, receiver)
            .await
            .map_err(|_| TaskGatewayError::Deadline)?
            .map_err(|_| TaskGatewayError::Server)?
    }
}
fn validate_identity(subject: &str, id: Option<&str>) -> Result<(), TaskGatewayError> {
    validate_task_identifier(subject)?;
    if let Some(id) = id {
        validate_task_identifier(id)?;
    }
    Ok(())
}
#[async_trait]
impl TaskBackend for RuntimeBackend {
    async fn send(
        &self,
        subject: &str,
        request: TaskRequest,
    ) -> Result<TaskSnapshot, TaskGatewayError> {
        validate_identity(subject, None)?;
        request.validate()?;
        self.request(|reply| Command::Send {
            subject: subject.to_string(),
            request,
            reply,
        })
        .await
    }
    async fn get(&self, subject: &str, id: &str) -> Result<TaskSnapshot, TaskGatewayError> {
        validate_identity(subject, Some(id))?;
        self.request(|reply| Command::Get {
            subject: subject.to_string(),
            id: id.to_string(),
            reply,
        })
        .await
    }
    async fn cancel(&self, subject: &str, id: &str) -> Result<TaskSnapshot, TaskGatewayError> {
        validate_identity(subject, Some(id))?;
        self.request(|reply| Command::Cancel {
            subject: subject.to_string(),
            id: id.to_string(),
            reply,
        })
        .await
    }
    async fn list(
        &self,
        subject: &str,
        request: TaskListRequest,
    ) -> Result<TaskList, TaskGatewayError> {
        validate_identity(subject, None)?;
        request.validate()?;
        self.request(|reply| Command::List {
            subject: subject.to_string(),
            request,
            reply,
        })
        .await
    }
    async fn subscribe(
        &self,
        subject: &str,
        id: &str,
    ) -> Result<TaskEventStream, TaskGatewayError> {
        validate_identity(subject, Some(id))?;
        self.request(|reply| Command::Subscribe {
            subject: subject.to_string(),
            id: id.to_string(),
            reply,
        })
        .await
    }
}
async fn run_owner(
    executor: Arc<dyn TaskExecutor>,
    max_parallel: usize,
    mut commands: mpsc::Receiver<Command>,
    mut stop: watch::Receiver<bool>,
) -> Result<(), TaskGatewayError> {
    let mut owner = Owner::new();
    loop {
        tokio::select! {biased;
            _=stop.changed()=>break,
            result=owner.jobs.join_next_with_id(),if !owner.jobs.is_empty()=>{if let Some(result)=result{owner.joined(result,None);}},
            command=commands.recv()=>{
                let Some(command)=command else{break;};
                handle_command(&mut owner,&executor,max_parallel,command);
            }
        }
    }
    commands.close();
    while let Ok(command) = commands.try_recv() {
        reject_command(command);
    }
    owner.drain().await
}
fn reject_command(command: Command) {
    match command {
        Command::Send { reply, .. }
        | Command::Get { reply, .. }
        | Command::Cancel { reply, .. } => {
            let _ = reply.send(Err(TaskGatewayError::Server));
        }
        Command::Subscribe { reply, .. } => {
            let _ = reply.send(Err(TaskGatewayError::Server));
        }
        Command::List { reply, .. } => {
            let _ = reply.send(Err(TaskGatewayError::Server));
        }
    }
}
/// Canonicalize only the ephemeral comparison value. The executor receives the
/// original request unchanged, including original numeric representations.
fn input_identity(request: &TaskRequest) -> Result<Vec<u8>, TaskGatewayError> {
    let payload = canonical_identity_payload(&request.payload)?;
    serde_json::to_vec(&(&request.context_id, &request.task_id, &payload))
        .map_err(|_| TaskGatewayError::InvalidRequest)
}
fn canonical_identity_payload(
    value: &serde_json::Value,
) -> Result<serde_json::Value, TaskGatewayError> {
    use serde_json::Value;
    match value {
        Value::Number(number) => {
            let integer = if let Some(integer) = number.as_i64() {
                Some(integer)
            } else if let Some(integer) = number.as_u64() {
                Some(i64::try_from(integer).map_err(|_| TaskGatewayError::InvalidRequest)?)
            } else {
                None
            };
            if let Some(integer) = integer {
                if !(-MAX_EXACT_INTEGER..=MAX_EXACT_INTEGER).contains(&integer) {
                    return Err(TaskGatewayError::InvalidRequest);
                }
                return Ok(Value::from(integer));
            }
            let float = number
                .as_f64()
                .filter(|value| value.is_finite())
                .ok_or(TaskGatewayError::InvalidRequest)?;
            if float.fract() == 0.0 {
                if float.abs() > MAX_EXACT_INTEGER_FLOAT {
                    return Err(TaskGatewayError::InvalidRequest);
                }
                let integer = format!("{float:.0}")
                    .parse::<i64>()
                    .map_err(|_| TaskGatewayError::InvalidRequest)?;
                Ok(Value::from(integer))
            } else {
                Ok(Value::Number(number.clone()))
            }
        }
        Value::Array(values) => values
            .iter()
            .map(canonical_identity_payload)
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Object(values) => {
            let sorted = values
                .iter()
                .map(|(key, value)| Ok((key.clone(), canonical_identity_payload(value)?)))
                .collect::<Result<BTreeMap<_, _>, TaskGatewayError>>()?;
            Ok(Value::Object(sorted.into_iter().collect()))
        }
        _ => Ok(value.clone()),
    }
}
fn handle_command(
    owner: &mut Owner,
    executor: &Arc<dyn TaskExecutor>,
    max_parallel: usize,
    command: Command,
) {
    match command {
        Command::Send {
            subject,
            request,
            reply,
        } => {
            let input = match input_identity(&request) {
                Ok(input) => input,
                Err(_) => {
                    let _ = reply.send(Err(TaskGatewayError::InvalidRequest));
                    return;
                }
            };
            if let Some(existing) = owner
                .entries
                .values()
                .find(|entry| entry.subject == subject && entry.key == request.idempotency_key)
            {
                let _ = reply.send(if existing.input == input {
                    Ok(existing.snapshot.clone())
                } else {
                    Err(TaskGatewayError::InvalidRequest)
                });
                return;
            }
            if request.task_id.is_some() {
                let _ = reply.send(Err(TaskGatewayError::Unsupported));
                return;
            }
            if owner.entries.len() >= MAX_RUNTIME_TASKS || owner.jobs.len() >= max_parallel {
                let _ = reply.send(Err(TaskGatewayError::Busy));
                return;
            }
            let id = uuid::Uuid::new_v4().to_string();
            let snapshot = TaskSnapshot {
                task_id: id.clone(),
                context_id: request.context_id.clone(),
                state: TaskState::Submitted,
                artifacts: vec![],
                error_code: None,
            };
            let (events, _) = broadcast::channel(EVENT_CAPACITY);
            let (cancel, cancellation) = watch::channel(false);
            let mut entry = Entry {
                subject: subject.clone(),
                key: request.idempotency_key.clone(),
                input,
                snapshot: snapshot.clone(),
                events,
                cancel,
                cancel_waiters: vec![],
            };
            let _ = reply.send(Ok(snapshot));
            entry.snapshot.state = TaskState::Working;
            publish(&entry);
            owner.entries.insert(id.clone(), entry);
            let executor = executor.clone();
            let execution_id = id.clone();
            let job = owner.jobs.spawn(async move {
                let result = executor.execute(&subject, request, cancellation).await;
                (execution_id, result)
            });
            owner.job_ids.insert(job.id(), id);
        }
        Command::Get { subject, id, reply } => {
            let _ = reply.send(
                owned_entry(&owner.entries, &subject, &id).map(|entry| entry.snapshot.clone()),
            );
        }
        Command::Cancel { subject, id, reply } => {
            match owner
                .entries
                .get_mut(&id)
                .filter(|entry| entry.subject == subject)
            {
                Some(entry) if entry.snapshot.state == TaskState::Canceled => {
                    let _ = reply.send(Ok(entry.snapshot.clone()));
                }
                Some(entry) if entry.snapshot.state.is_terminal() => {
                    let _ = reply.send(Err(TaskGatewayError::Unsupported));
                }
                Some(entry) => {
                    entry.cancel_waiters.retain(|waiter| !waiter.is_closed());
                    if entry.cancel_waiters.len() >= MAX_CANCEL_WAITERS {
                        let _ = reply.send(Err(TaskGatewayError::Busy));
                    } else {
                        entry.cancel.send_replace(true);
                        entry.cancel_waiters.push(reply);
                    }
                }
                None => {
                    let _ = reply.send(Err(TaskGatewayError::NotFound));
                }
            }
        }
        Command::List {
            subject,
            request,
            reply,
        } => {
            let _ = reply.send(list_tasks(&owner.entries, &subject, request));
        }
        Command::Subscribe { subject, id, reply } => {
            let result = owned_entry(&owner.entries, &subject, &id).and_then(|entry| {
                if entry.events.receiver_count() >= MAX_SUBSCRIPTIONS {
                    return Err(TaskGatewayError::Busy);
                }
                // Subscribe and snapshot occur within this single actor turn.
                // Terminal initial snapshots close the internal unary-wait race.
                Ok(snapshot_stream(
                    entry.snapshot.clone(),
                    entry.events.subscribe(),
                ))
            });
            let _ = reply.send(result);
        }
    }
}
fn list_tasks(
    entries: &BTreeMap<String, Entry>,
    subject: &str,
    request: TaskListRequest,
) -> Result<TaskList, TaskGatewayError> {
    let matching = entries
        .values()
        .filter(|entry| {
            entry.subject == subject
                && request
                    .context_id
                    .as_ref()
                    .is_none_or(|context| context == &entry.snapshot.context_id)
                && request
                    .state
                    .is_none_or(|state| state == entry.snapshot.state)
        })
        .collect::<Vec<_>>();
    let offset = request
        .page_token
        .as_deref()
        .map(str::parse::<usize>)
        .transpose()
        .map_err(|_| TaskGatewayError::InvalidRequest)?
        .unwrap_or(0);
    if offset > matching.len() {
        return Err(TaskGatewayError::InvalidRequest);
    }
    let mut page = TaskList {
        tasks: vec![],
        next_page_token: None,
        total_size: matching.len() as u32,
    };
    let limit = offset
        .saturating_add(request.page_size as usize)
        .min(matching.len());
    let mut end = offset;
    for entry in &matching[offset..limit] {
        let mut snapshot = entry.snapshot.clone();
        if !request.include_artifacts {
            snapshot.artifacts.clear();
        }
        page.tasks.push(snapshot);
        let next = end + 1;
        page.next_page_token = (next < matching.len()).then(|| next.to_string());
        if page.validate().is_err() {
            page.tasks.pop();
            if page.tasks.is_empty() {
                return Err(TaskGatewayError::Busy);
            }
            break;
        }
        end = next;
    }
    page.next_page_token = (end < matching.len()).then(|| end.to_string());
    page.validate()?;
    Ok(page)
}
fn snapshot_stream(
    initial: TaskSnapshot,
    receiver: broadcast::Receiver<TaskSnapshot>,
) -> TaskEventStream {
    Box::pin(stream::unfold(
        (Some(initial), receiver, false),
        |(initial, mut receiver, done)| async move {
            if done {
                return None;
            }
            if let Some(snapshot) = initial {
                let done = snapshot.state.is_terminal();
                return Some((Ok(snapshot), (None, receiver, done)));
            }
            match receiver.recv().await {
                Ok(snapshot) => {
                    let done = snapshot.state.is_terminal();
                    Some((Ok(snapshot), (None, receiver, done)))
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    Some((Err(TaskGatewayError::Busy), (None, receiver, true)))
                }
                Err(_) => Some((Err(TaskGatewayError::Server), (None, receiver, true))),
            }
        },
    ))
}
fn owned_entry<'a>(
    entries: &'a BTreeMap<String, Entry>,
    subject: &str,
    id: &str,
) -> Result<&'a Entry, TaskGatewayError> {
    entries
        .get(id)
        .filter(|entry| entry.subject == subject)
        .ok_or(TaskGatewayError::NotFound)
}
fn publish(entry: &Entry) {
    let _ = entry.events.send(entry.snapshot.clone());
}
fn finish(
    entries: &mut BTreeMap<String, Entry>,
    id: &str,
    result: Result<TaskExecution, TaskGatewayError>,
) {
    if let Some(entry) = entries.get_mut(id) {
        if entry.snapshot.state.is_terminal() {
            return;
        }
        match result {
            Ok(TaskExecution::Canceled) => {
                entry.snapshot.state = TaskState::Canceled;
                entry.snapshot.error_code = None;
            }
            Ok(TaskExecution::Completed(artifacts)) => {
                // Only the executor knows whether completion was verified before a
                // racing cancel signal. A late signal cannot relabel verified work.
                entry.snapshot.artifacts = artifacts;
                entry.snapshot.state = TaskState::Completed;
                entry.snapshot.error_code = None;
                if entry.snapshot.validate().is_err() {
                    entry.snapshot.artifacts.clear();
                    entry.snapshot.state = TaskState::Failed;
                    entry.snapshot.error_code = Some(TaskGatewayError::Protocol.code().into());
                }
            }
            Err(error) => {
                entry.snapshot.state = TaskState::Failed;
                entry.snapshot.error_code = Some(error.code().into());
            }
        }
        publish(entry);
        for reply in entry.cancel_waiters.drain(..) {
            let _ = reply.send(Ok(entry.snapshot.clone()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    #[tokio::test]
    async fn subscriber_lag_is_explicit_and_terminal() {
        let initial = TaskSnapshot {
            task_id: "task".into(),
            context_id: "context".into(),
            state: TaskState::Working,
            artifacts: vec![],
            error_code: None,
        };
        let (sender, receiver) = broadcast::channel(EVENT_CAPACITY);
        let mut stream = snapshot_stream(initial.clone(), receiver);
        assert!(stream.next().await.expect("initial").is_ok());
        for _ in 0..=EVENT_CAPACITY {
            sender.send(initial.clone()).expect("subscriber");
        }
        assert_eq!(
            stream.next().await.expect("lag report").expect_err("lag"),
            TaskGatewayError::Busy
        );
        assert!(stream.next().await.is_none());
    }
}
