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
    use serde_json::{Value, json};
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

    const REFUSED: TaskGatewayError = TaskGatewayError::InvalidRequest;

    #[test]
    fn the_identity_payload_is_canonical() {
        for (input, expected) in [
            (json!(1.0), json!(1)),
            (json!(-0.0), json!(0)),
            (json!(1e3), json!(1000)),
            (json!(-2.0), json!(-2)),
            (json!(1.5), json!(1.5)),
            (json!(MAX_EXACT_INTEGER), json!(MAX_EXACT_INTEGER)),
            (json!(-MAX_EXACT_INTEGER), json!(-MAX_EXACT_INTEGER)),
            (json!(MAX_EXACT_INTEGER_FLOAT), json!(MAX_EXACT_INTEGER)),
            (json!(-MAX_EXACT_INTEGER_FLOAT), json!(-MAX_EXACT_INTEGER)),
            (json!("1.0"), json!("1.0")),
            (json!(true), json!(true)),
            (json!(null), json!(null)),
        ] {
            assert_eq!(canonical_identity_payload(&input), Ok(expected), "{input}");
        }
        let nested = canonical_identity_payload(&json!([1.0, [2.0], {"b": 3.0, "a": [4.0]}]))
            .expect("nested payload");
        assert_eq!(nested.to_string(), r#"[1,[2],{"a":[4],"b":3}]"#);

        for invalid in [
            json!(MAX_EXACT_INTEGER + 1),
            json!(-MAX_EXACT_INTEGER - 1),
            json!(u64::MAX),
            json!(9_007_199_254_740_992.0),
            json!(-9_007_199_254_740_992.0),
            json!(1e300),
            json!([1, [MAX_EXACT_INTEGER + 1]]),
            json!({"a": {"b": u64::MAX}}),
        ] {
            assert_eq!(
                canonical_identity_payload(&invalid),
                Err(REFUSED),
                "{invalid}"
            );
        }
    }

    #[test]
    fn the_input_identity_ignores_request_ids_and_number_spelling() {
        let request = |payload: Value| TaskRequest {
            request_id: "request_1".to_string(),
            context_id: "context_1".to_string(),
            task_id: Some("task_1".to_string()),
            idempotency_key: "key_1".to_string(),
            payload,
        };
        let identity = |request: &TaskRequest| input_identity(request).expect("identity");
        let base = request(json!({"goal": "review", "limit": 2}));
        for same in [
            TaskRequest {
                request_id: "request_2".to_string(),
                idempotency_key: "key_2".to_string(),
                ..base.clone()
            },
            request(json!({"limit": 2.0, "goal": "review"})),
        ] {
            assert_eq!(identity(&same), identity(&base), "{same:?}");
        }
        for different in [
            TaskRequest {
                context_id: "context_2".to_string(),
                ..base.clone()
            },
            TaskRequest {
                task_id: None,
                ..base.clone()
            },
            TaskRequest {
                task_id: Some("task_2".to_string()),
                ..base.clone()
            },
            request(json!({"goal": "review", "limit": 2.5})),
            request(json!({"goal": "fix", "limit": 2})),
        ] {
            assert_ne!(identity(&different), identity(&base), "{different:?}");
        }
        assert_eq!(
            input_identity(&request(json!({"limit": u64::MAX}))),
            Err(REFUSED)
        );
    }

    fn entry(
        subject: &str,
        context_id: &str,
        state: TaskState,
        artifacts: Vec<TaskArtifact>,
    ) -> Entry {
        Entry {
            subject: subject.to_string(),
            key: String::new(),
            input: Vec::new(),
            snapshot: TaskSnapshot {
                task_id: String::new(),
                context_id: context_id.to_string(),
                state,
                artifacts,
                error_code: None,
            },
            events: broadcast::channel(EVENT_CAPACITY).0,
            cancel: watch::channel(false).0,
            cancel_waiters: Vec::new(),
        }
    }

    fn entries(list: Vec<(&str, Entry)>) -> BTreeMap<String, Entry> {
        list.into_iter()
            .map(|(id, mut entry)| {
                entry.snapshot.task_id = id.to_string();
                (id.to_string(), entry)
            })
            .collect()
    }

    fn list_request(page_size: u32, page_token: Option<&str>) -> TaskListRequest {
        TaskListRequest {
            context_id: None,
            state: None,
            page_size,
            page_token: page_token.map(str::to_string),
            include_artifacts: false,
        }
    }

    /// The task ids, the next page token, and the total size of a listing.
    fn page(list: Result<TaskList, TaskGatewayError>) -> (Vec<String>, Option<String>, u32) {
        let list = list.expect("list");
        let ids = list.tasks.iter().map(|task| task.task_id.clone()).collect();
        (ids, list.next_page_token, list.total_size)
    }

    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    /// A data artifact of about 12 KiB: two fit a 32 KiB listing, three do not.
    fn large_artifact() -> TaskArtifact {
        TaskArtifact::data("artifact_1", json!("x".repeat(12 * 1024)))
    }

    #[test]
    fn a_listing_keeps_only_the_callers_matching_tasks() {
        let entries = entries(vec![
            (
                "task_1",
                entry("alice", "context_a", TaskState::Working, vec![]),
            ),
            (
                "task_2",
                entry("bob", "context_a", TaskState::Working, vec![]),
            ),
            (
                "task_3",
                entry("alice", "context_b", TaskState::Completed, vec![]),
            ),
            (
                "task_4",
                entry("alice", "context_a", TaskState::Completed, vec![]),
            ),
            (
                "task_5",
                entry("alice", "context_a", TaskState::Working, vec![]),
            ),
        ]);
        let filtered = |subject: &str, context_id: Option<&str>, state: Option<TaskState>| {
            let request = TaskListRequest {
                context_id: context_id.map(str::to_string),
                state,
                ..list_request(32, None)
            };
            page(list_tasks(&entries, subject, request))
        };
        assert_eq!(
            filtered("alice", None, None),
            (ids(&["task_1", "task_3", "task_4", "task_5"]), None, 4)
        );
        assert_eq!(filtered("bob", None, None), (ids(&["task_2"]), None, 1));
        assert_eq!(filtered("carol", None, None), (ids(&[]), None, 0));
        assert_eq!(
            filtered("alice", Some("context_a"), None),
            (ids(&["task_1", "task_4", "task_5"]), None, 3)
        );
        assert_eq!(
            filtered("alice", None, Some(TaskState::Completed)),
            (ids(&["task_3", "task_4"]), None, 2)
        );
        assert_eq!(
            filtered("alice", Some("context_a"), Some(TaskState::Working)),
            (ids(&["task_1", "task_5"]), None, 2)
        );
    }

    #[test]
    fn a_listing_pages_by_a_numeric_offset_token() {
        let entries = entries(
            ["task_1", "task_2", "task_3", "task_4"]
                .into_iter()
                .map(|id| (id, entry("alice", "context_a", TaskState::Working, vec![])))
                .collect(),
        );
        let listed = |page_size: u32, page_token: Option<&str>| {
            list_tasks(&entries, "alice", list_request(page_size, page_token))
        };
        let two = Some("2".to_string());
        assert_eq!(page(listed(2, None)), (ids(&["task_1", "task_2"]), two, 4));
        assert_eq!(
            page(listed(2, Some("2"))),
            (ids(&["task_3", "task_4"]), None, 4)
        );
        assert_eq!(
            page(listed(2, Some("1"))),
            (ids(&["task_2", "task_3"]), Some("3".to_string()), 4)
        );
        assert_eq!(
            page(listed(3, Some("2"))),
            (ids(&["task_3", "task_4"]), None, 4)
        );
        assert_eq!(page(listed(2, Some("4"))), (ids(&[]), None, 4));
        for invalid in ["5", "x", "-1", ""] {
            assert_eq!(listed(2, Some(invalid)), Err(REFUSED), "{invalid:?}");
        }
    }

    #[test]
    fn a_listing_strips_artifacts_unless_asked_and_stops_at_its_size_bound() {
        let large_entries = entries(
            ["task_1", "task_2", "task_3"]
                .into_iter()
                .map(|id| {
                    (
                        id,
                        entry(
                            "alice",
                            "context_a",
                            TaskState::Working,
                            vec![large_artifact()],
                        ),
                    )
                })
                .collect(),
        );
        let listed = |entries: &BTreeMap<String, Entry>,
                      page_token: Option<&str>,
                      include_artifacts: bool| {
            let request = TaskListRequest {
                include_artifacts,
                ..list_request(32, page_token)
            };
            list_tasks(entries, "alice", request)
        };
        let stripped = listed(&large_entries, None, false).expect("stripped list");
        assert_eq!(stripped.tasks.len(), 3);
        assert!(stripped.tasks.iter().all(|task| task.artifacts.is_empty()));
        assert_eq!(large_entries["task_1"].snapshot.artifacts.len(), 1);

        let first = listed(&large_entries, None, true).expect("first page");
        assert!(
            first
                .tasks
                .iter()
                .all(|task| task.artifacts == [large_artifact()])
        );
        assert_eq!(
            page(Ok(first)),
            (ids(&["task_1", "task_2"]), Some("2".to_string()), 3)
        );
        assert_eq!(
            page(listed(&large_entries, Some("2"), true)),
            (ids(&["task_3"]), None, 3)
        );

        // A snapshot of exactly 32 KiB is valid alone but cannot fit a listing. The page
        // ends before it rather than skipping it for a later task that would fit.
        let mut full = TaskSnapshot {
            task_id: "task_2".to_string(),
            context_id: "context_a".to_string(),
            state: TaskState::Working,
            artifacts: vec![
                TaskArtifact::data("artifact_1", json!("x".repeat(MAX_TASK_PAYLOAD_BYTES - 2))),
                TaskArtifact::data("artifact_2", json!("")),
            ],
            error_code: None,
        };
        let missing = MAX_TASK_WIRE_BYTES / 2 - serde_json::to_vec(&full).expect("size").len();
        full.artifacts[1] = TaskArtifact::data("artifact_2", json!("x".repeat(missing)));
        assert_eq!(
            serde_json::to_vec(&full).expect("size").len(),
            MAX_TASK_WIRE_BYTES / 2
        );
        assert_eq!(full.validate(), Ok(()));
        let full_entries = entries(vec![
            (
                "task_1",
                entry("alice", "context_a", TaskState::Working, vec![]),
            ),
            (
                "task_2",
                entry("alice", "context_a", TaskState::Working, full.artifacts),
            ),
            (
                "task_3",
                entry("alice", "context_a", TaskState::Working, vec![]),
            ),
        ]);
        assert_eq!(
            page(listed(&full_entries, None, true)),
            (ids(&["task_1"]), Some("1".to_string()), 3)
        );
        assert_eq!(
            listed(&full_entries, Some("1"), true),
            Err(TaskGatewayError::Busy)
        );
        assert_eq!(
            page(listed(&full_entries, Some("1"), false)),
            (ids(&["task_2", "task_3"]), None, 3)
        );
    }
}
