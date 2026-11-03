//! One bounded command owner serializes process admission, sends and lifecycle.
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use time::OffsetDateTime;
use tokio::{
    sync::{OnceCell, mpsc, oneshot, watch},
    task::{JoinHandle, JoinSet},
};
use uuid::Uuid;
use vibemux_harness::chat_session::{AssistantText, SessionLimits, SessionTarget, TurnRequest};
use vibemux_store::{
    ChatExecutionAttemptCommit, ChatExecutionOwnerCommit, ChatExecutionSessionCommit, ChatMutation,
    ChatOpen, ChatOwnerMutation, ChatSessionCommit, ChatTurnCommit,
};
use vibemux_types::chat_command_progress::{CommandProgressScope, CommandProgressSnapshot};
use vibemux_types::{
    ChatId,
    chat::{ChatError, ChatHarness, ChatSession, ChatState, ChatTurn, ChatTurnPhase},
};

use super::{
    ChatExecutionSink, ChatWriterSink, RuntimeBinding, RuntimeError, RuntimeLimits, RuntimeOwner,
    RuntimePlan, RuntimeStopReason, spawn_runtime,
};
use crate::{
    WriterHandle,
    chat_writer::{ChatWriteError, ChatWriter, PrivateChatRecord},
    harness_dispatch::{
        DispatchServiceError, HarnessDispatchService, chat_launch::PreparedChat, git_head,
    },
};

const COMMAND_CAPACITY: usize = 32;
const REQUEST_DEADLINE: Duration = Duration::from_secs(30);
const SERVICE_SHUTDOWN: Duration = Duration::from_secs(60);
// A live provisioning job awaits the bounded Git operations through its receipt.
const WORKSPACE_SERVICE_SHUTDOWN: Duration = Duration::from_secs(600);
const _: () = assert!(
    crate::DEFAULT_SHUTDOWN_OBSERVATION_TIMEOUT.as_secs() > WORKSPACE_SERVICE_SHUTDOWN.as_secs()
);

#[path = "approval_service.rs"]
mod approval_service;
#[path = "execution_service.rs"]
mod execution_service;
#[path = "owner_service.rs"]
mod owner_service;

#[derive(Debug, thiserror::Error)]
pub enum ChatServiceError {
    #[error(transparent)]
    Writer(#[from] ChatWriteError),
    #[error(transparent)]
    Dispatch(#[from] DispatchServiceError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error(transparent)]
    Workspace(#[from] vibemux_workspace::WorkspaceError),
    #[error(transparent)]
    Rejected(#[from] ChatError),
    #[error("chat service is stopped")]
    Stopped,
    #[error("chat service queue is full")]
    QueueFull,
    #[error("chat operation acknowledgement is uncertain")]
    AcknowledgementUncertain,
    #[error("chat shutdown incomplete: {code}")]
    Shutdown { code: String },
}
impl ChatServiceError {
    pub fn code(&self) -> &str {
        match self {
            Self::Writer(error) => error.code(),
            Self::Dispatch(error) => error.code(),
            Self::Rejected(error) => error.code(),
            Self::Runtime(_) => "chat_runtime_failed",
            Self::Workspace(error) => error.code(),
            Self::Stopped => "chat_service_stopped",
            Self::QueueFull => "chat_queue_full",
            Self::AcknowledgementUncertain => "chat_acknowledgement_uncertain",
            Self::Shutdown { code } => code,
        }
    }
}

#[derive(Clone, Copy)]
pub enum ChatSessionAction {
    Close,
    Archive,
    Resume,
    Purge,
}

enum Action {
    #[cfg(feature = "test_helpers")]
    StartFixtureApproval(Box<approval_service::FixtureApprovalStart>),
    ApprovalHandle {
        chat_id: ChatId,
        generation: u64,
        request_id: Uuid,
    },
    StartExecution {
        input: ChatOwnerMutation,
        generation: u64,
    },
    SendExecution {
        chat_id: ChatId,
        generation: u64,
        owner_version: u64,
        request_id: Uuid,
        prompt: String,
    },
    CancelExecution {
        chat_id: ChatId,
        generation: u64,
        request_id: Uuid,
    },
    CloseExecutionProcess {
        chat_id: ChatId,
        generation: u64,
    },
    OpenExecutionOwner {
        operation_id: Uuid,
        harness: ChatHarness,
        title: Option<String>,
    },
    ReconcileExecutionOwner {
        input: ChatOwnerMutation,
    },
    StopExecutionOwner {
        input: ChatOwnerMutation,
    },
    Open {
        operation_id: Uuid,
        harness: ChatHarness,
        title: Option<String>,
    },
    Send {
        chat_id: ChatId,
        generation: u64,
        request_id: Uuid,
        prompt: String,
    },
    Cancel {
        chat_id: ChatId,
        generation: u64,
        request_id: Uuid,
    },
    Rename {
        input: ChatMutation,
        title: String,
    },
    Session {
        input: ChatMutation,
        action: ChatSessionAction,
    },
    LiveText {
        chat_id: ChatId,
        generation: u64,
        request_id: Uuid,
    },
    CommandProgress {
        scope: CommandProgressScope,
        chat_id: ChatId,
        generation: u64,
        request_id: Uuid,
    },
}
enum Reply {
    #[cfg(feature = "test_helpers")]
    FixtureApprovalHandle(super::RuntimeHandle),
    ApprovalHandle(Option<Box<approval_service::LiveApproval>>),
    ExecutionProcess(Box<ChatExecutionSessionCommit>),
    ExecutionAttempt(Box<ChatExecutionAttemptCommit>),
    CancelledExecution(Box<vibemux_types::chat_attempt::ChatExecutionAttempt>),
    ExecutionOwner(Box<ChatExecutionOwnerCommit>),
    Session(Box<ChatSessionCommit>),
    Turn(Box<ChatTurnCommit>),
    PrivateText(Option<AssistantText>),
    CommandProgress(Option<CommandProgressSnapshot>),
}
struct Command {
    action: Action,
    response: oneshot::Sender<Result<Reply, ChatServiceError>>,
}

struct Context {
    source: WriterHandle,
    state_dir: PathBuf,
    writer: OnceCell<ChatWriter>,
    dispatch: HarnessDispatchService,
    pending: Mutex<HashMap<ChatId, RuntimeBinding>>,
    provisioning: Mutex<Option<ChatId>>,
    stopping: watch::Receiver<bool>,
}
impl Context {
    async fn has_project(&self) -> Result<bool, ChatServiceError> {
        let source = self.source.clone();
        Ok(tokio::task::spawn_blocking(move || source.project_id())
            .await
            .map_err(|_| ChatServiceError::Stopped)?
            .map_err(ChatWriteError::from)?
            .is_some())
    }
    async fn writer(&self) -> Result<&ChatWriter, ChatServiceError> {
        self.writer
            .get_or_try_init(|| ChatWriter::attach(self.source.clone(), self.state_dir.clone()))
            .await
            .map_err(Into::into)
    }
    async fn reconcile_pending(&self) -> Result<(), ChatServiceError> {
        let pending: Vec<_> = self
            .pending
            .lock()
            .map_err(|_| ChatServiceError::Stopped)?
            .values()
            .cloned()
            .collect();
        if pending.is_empty() {
            return Ok(());
        }
        let writer = self.writer().await?;
        for binding in pending {
            if let Some(stored) = writer.execution_owner(binding.chat_id).await? {
                if stored.session.generation == binding.generation
                    && matches!(
                        stored.session.state,
                        ChatState::Opening | ChatState::Ready | ChatState::Busy
                    )
                {
                    writer
                        .change_execution_process(
                            mutation(binding.chat_id, binding.generation),
                            binding.owner_fence,
                            vibemux_store::ChatExecutionProcessChange::Released {
                                cleanup_verified: false,
                                closed: false,
                            },
                        )
                        .await?;
                }
                self.pending
                    .lock()
                    .map_err(|_| ChatServiceError::Stopped)?
                    .remove(&binding.chat_id);
                continue;
            }
            let current = writer
                .session(binding.chat_id)
                .await?
                .ok_or(ChatError::NotFound)?;
            if current.generation == binding.generation
                && matches!(
                    current.state,
                    ChatState::Opening | ChatState::Ready | ChatState::Busy
                )
            {
                writer
                    .owner_lost(binding.chat_id, binding.generation, binding.owner_fence)
                    .await?;
            }
            self.pending
                .lock()
                .map_err(|_| ChatServiceError::Stopped)?
                .remove(&binding.chat_id);
        }
        Ok(())
    }
}
struct Inner {
    context: Arc<Context>,
    commands: mpsc::Sender<Command>,
    stop: watch::Sender<bool>,
    task: Mutex<Option<JoinHandle<Result<(), ChatServiceError>>>>,
    completion: watch::Receiver<Option<Result<(), String>>>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}

#[derive(Clone)]
pub struct ChatService {
    inner: Arc<Inner>,
}
impl ChatService {
    pub fn begin_shutdown(&self) {
        let _ = self.inner.stop.send(true);
    }
    /// Startup creates no private storage and launches no vendor process.
    pub fn new(source: WriterHandle, dispatch: HarnessDispatchService, state_dir: PathBuf) -> Self {
        let (stop, stopping) = watch::channel(false);
        let context = Arc::new(Context {
            source,
            state_dir,
            writer: OnceCell::new(),
            dispatch,
            pending: Mutex::new(HashMap::new()),
            provisioning: Mutex::new(None),
            stopping: stopping.clone(),
        });
        let (commands, receiver) = mpsc::channel(COMMAND_CAPACITY);
        let actor = Actor {
            context: Arc::clone(&context),
            owners: HashMap::new(),
            provisions: JoinSet::new(),
        };
        let (complete, completion) = watch::channel(None);
        let task = tokio::spawn(async move {
            let result = actor.run(receiver, stopping).await;
            complete.send_replace(Some(
                result
                    .as_ref()
                    .map(|_| ())
                    .map_err(|error| error.code().to_owned()),
            ));
            result
        });
        Self {
            inner: Arc::new(Inner {
                context,
                commands,
                stop,
                task: Mutex::new(Some(task)),
                completion,
            }),
        }
    }
    async fn request(&self, action: Action) -> Result<Reply, ChatServiceError> {
        if *self.inner.stop.borrow() {
            return Err(ChatServiceError::Stopped);
        }
        let (response, receiver) = oneshot::channel();
        self.inner
            .commands
            .try_send(Command { action, response })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => ChatServiceError::QueueFull,
                mpsc::error::TrySendError::Closed(_) => ChatServiceError::Stopped,
            })?;
        // Dropping/timing out the caller never drops an admitted command.
        tokio::time::timeout(REQUEST_DEADLINE, receiver)
            .await
            .map_err(|_| ChatServiceError::AcknowledgementUncertain)?
            .map_err(|_| ChatServiceError::AcknowledgementUncertain)?
    }
    pub async fn open(
        &self,
        operation_id: Uuid,
        harness: ChatHarness,
        title: Option<String>,
    ) -> Result<ChatSessionCommit, ChatServiceError> {
        match self
            .request(Action::Open {
                operation_id,
                harness,
                title,
            })
            .await?
        {
            Reply::Session(value) => Ok(*value),
            _ => Err(ChatServiceError::Stopped),
        }
    }
    pub async fn send(
        &self,
        chat_id: ChatId,
        generation: u64,
        request_id: Uuid,
        prompt: String,
    ) -> Result<ChatTurnCommit, ChatServiceError> {
        match self
            .request(Action::Send {
                chat_id,
                generation,
                request_id,
                prompt,
            })
            .await?
        {
            Reply::Turn(value) => Ok(*value),
            _ => Err(ChatServiceError::Stopped),
        }
    }
    pub async fn cancel(
        &self,
        chat_id: ChatId,
        generation: u64,
        request_id: Uuid,
    ) -> Result<ChatTurnCommit, ChatServiceError> {
        match self
            .request(Action::Cancel {
                chat_id,
                generation,
                request_id,
            })
            .await?
        {
            Reply::Turn(value) => Ok(*value),
            _ => Err(ChatServiceError::Stopped),
        }
    }
    pub async fn rename(
        &self,
        input: ChatMutation,
        title: String,
    ) -> Result<ChatSessionCommit, ChatServiceError> {
        match self.request(Action::Rename { input, title }).await? {
            Reply::Session(value) => Ok(*value),
            _ => Err(ChatServiceError::Stopped),
        }
    }
    pub async fn change_session(
        &self,
        input: ChatMutation,
        action: ChatSessionAction,
    ) -> Result<ChatSessionCommit, ChatServiceError> {
        match self.request(Action::Session { input, action }).await? {
            Reply::Session(value) => Ok(*value),
            _ => Err(ChatServiceError::Stopped),
        }
    }
    pub async fn sessions(
        &self,
        after: Option<ChatId>,
        limit: u32,
        archived: bool,
    ) -> Result<Vec<ChatSession>, ChatServiceError> {
        if limit == 0 || limit > vibemux_types::chat::MAX_CHAT_PAGE {
            return Err(ChatError::InvalidInput.into());
        }
        // A fresh database has no content and no project identity yet.
        let source = self.inner.context.source.clone();
        let known = tokio::task::spawn_blocking(move || source.project_id())
            .await
            .map_err(|_| ChatServiceError::Stopped)?
            .map_err(ChatWriteError::from)?;
        if known.is_none() {
            return Ok(Vec::new());
        }
        self.inner
            .context
            .writer()
            .await?
            .sessions(after, limit, archived)
            .await
            .map_err(Into::into)
    }
    pub async fn session(&self, id: ChatId) -> Result<Option<ChatSession>, ChatServiceError> {
        self.inner
            .context
            .writer()
            .await?
            .session(id)
            .await
            .map_err(Into::into)
    }
    pub async fn session_for_operation(
        &self,
        id: Uuid,
    ) -> Result<Option<ChatSession>, ChatServiceError> {
        self.inner
            .context
            .writer()
            .await?
            .session_for_operation(id)
            .await
            .map_err(Into::into)
    }
    pub async fn turn(&self, id: Uuid) -> Result<Option<ChatTurn>, ChatServiceError> {
        self.inner
            .context
            .writer()
            .await?
            .turn(id)
            .await
            .map_err(Into::into)
    }
    pub async fn turns(
        &self,
        id: ChatId,
        after: u64,
        limit: u32,
    ) -> Result<Vec<ChatTurn>, ChatServiceError> {
        self.inner
            .context
            .writer()
            .await?
            .turns(id, after, limit)
            .await
            .map_err(Into::into)
    }
    pub async fn records(
        &self,
        id: ChatId,
        after: u64,
        limit: u32,
    ) -> Result<Vec<PrivateChatRecord>, ChatServiceError> {
        self.inner
            .context
            .writer()
            .await?
            .records(id, after, limit)
            .await
            .map_err(Into::into)
    }
    pub async fn title(&self, id: ChatId) -> Result<Option<String>, ChatServiceError> {
        self.inner
            .context
            .writer()
            .await?
            .title(id)
            .await
            .map_err(Into::into)
    }
    /// Explicit private replacement snapshot; status and events contain no text.
    pub async fn live_text(
        &self,
        chat_id: ChatId,
        generation: u64,
        request_id: Uuid,
    ) -> Result<Option<AssistantText>, ChatServiceError> {
        match self
            .request(Action::LiveText {
                chat_id,
                generation,
                request_id,
            })
            .await?
        {
            Reply::PrivateText(value) => Ok(value),
            _ => Err(ChatServiceError::Stopped),
        }
    }
    pub async fn command_progress(
        &self,
        scope: CommandProgressScope,
        chat_id: ChatId,
        generation: u64,
        request_id: Uuid,
    ) -> Result<Option<CommandProgressSnapshot>, ChatServiceError> {
        match self
            .request(Action::CommandProgress {
                scope,
                chat_id,
                generation,
                request_id,
            })
            .await?
        {
            Reply::CommandProgress(value) => Ok(value),
            _ => Err(ChatServiceError::Stopped),
        }
    }

    pub async fn shutdown(&self) -> Result<(), ChatServiceError> {
        self.begin_shutdown();
        let mut completion = self.inner.completion.clone();
        let bound = if self
            .inner
            .context
            .provisioning
            .lock()
            .map_err(|_| ChatServiceError::Stopped)?
            .is_some()
        {
            WORKSPACE_SERVICE_SHUTDOWN
        } else {
            SERVICE_SHUTDOWN
        };
        tokio::time::timeout(bound, async {
            let outcome = loop {
                if let Some(result) = completion.borrow().clone() {
                    break result.map_err(|code| ChatServiceError::Shutdown { code });
                }
                if completion.changed().await.is_err() {
                    break Err(ChatServiceError::Stopped);
                }
            };
            loop {
                let (task, exists) = {
                    let mut task = self
                        .inner
                        .task
                        .lock()
                        .map_err(|_| ChatServiceError::Stopped)?;
                    let exists = task.is_some();
                    let finished = task.as_ref().is_some_and(JoinHandle::is_finished);
                    (if finished { task.take() } else { None }, exists)
                };
                if let Some(task) = task {
                    // A finished handle returns Ready without a cancellable wait.
                    return task.await.map_err(|_| ChatServiceError::Stopped)?;
                }
                if !exists {
                    return outcome;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| ChatServiceError::AcknowledgementUncertain)?
    }
}

struct OwnedRuntime {
    owner: RuntimeOwner,
    binding: RuntimeBinding,
    root: PathBuf,
    deadline: Duration,
    sink: RuntimeWriterSink,
}
enum RuntimeWriterSink {
    ReadOnly(Arc<ChatWriterSink>),
    Execution(Arc<ChatExecutionSink>),
}
impl RuntimeWriterSink {
    fn command_progress(
        &self,
        scope: CommandProgressScope,
        request_id: Uuid,
    ) -> Result<Option<CommandProgressSnapshot>, ChatServiceError> {
        match (scope, self) {
            (CommandProgressScope::Saved, Self::ReadOnly(sink)) => {
                sink.command_progress(request_id).map_err(Into::into)
            }
            (CommandProgressScope::Workspace, Self::Execution(sink)) => {
                sink.command_progress(request_id).map_err(Into::into)
            }
            _ => Err(ChatError::InvalidInput.into()),
        }
    }
    fn live_text(&self, request_id: Uuid) -> Option<AssistantText> {
        match self {
            Self::ReadOnly(sink) => sink.live_text(request_id),
            Self::Execution(sink) => sink.live_text(request_id),
        }
    }
}
struct Actor {
    context: Arc<Context>,
    owners: HashMap<ChatId, OwnedRuntime>,
    provisions: JoinSet<Result<(), ChatServiceError>>,
}
impl Actor {
    async fn run(
        mut self,
        mut commands: mpsc::Receiver<Command>,
        mut stop: watch::Receiver<bool>,
    ) -> Result<(), ChatServiceError> {
        loop {
            tokio::select! {biased;
                changed=stop.changed()=>{if changed.is_err()||*stop.borrow(){break;}}
                result=self.provisions.join_next(), if !self.provisions.is_empty()=>{
                    self.finish_provision(result).await;
                }
                command=commands.recv()=>{
                    let Some(command)=command else {break;};
                    self.reap_finished().await;
                    let result=self.execute(command.action).await;
                    let _=command.response.send(result);
                }
            }
        }
        commands.close();
        while let Ok(command) = commands.try_recv() {
            let _ = command.response.send(Err(ChatServiceError::Stopped));
        }
        for owned in self.owners.values() {
            owned.owner.handle.request_stop(RuntimeStopReason::Shutdown);
        }
        let mut failure = None;
        while let Some(result) = self.provisions.join_next().await {
            self.finish_provision(Some(result)).await;
        }
        for (_, owned) in self.owners.drain() {
            if let Err(error) = end_owner(&self.context, owned).await {
                failure = Some(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }
    async fn reap_finished(&mut self) {
        let finished: Vec<_> = self
            .owners
            .iter()
            .filter_map(|(id, owned)| owned.owner.task.is_finished().then_some(*id))
            .collect();
        for id in finished {
            if let Some(owned) = self.owners.remove(&id) {
                let _ = end_owner(&self.context, owned).await;
            }
        }
    }
    async fn execute(&mut self, action: Action) -> Result<Reply, ChatServiceError> {
        self.context.reconcile_pending().await?;
        match action {
            #[cfg(feature = "test_helpers")]
            Action::StartFixtureApproval(input) => self
                .start_fixture_approval(*input)
                .await
                .map(Reply::FixtureApprovalHandle),
            Action::ApprovalHandle {
                chat_id,
                generation,
                request_id,
            } => Ok(Reply::ApprovalHandle(
                self.approval_handle(chat_id, generation, request_id),
            )),
            Action::StartExecution { input, generation } => self
                .start_execution(input, generation)
                .await
                .map(Box::new)
                .map(Reply::ExecutionProcess),
            Action::SendExecution {
                chat_id,
                generation,
                owner_version,
                request_id,
                prompt,
            } => self
                .send_execution(chat_id, generation, owner_version, request_id, prompt)
                .await
                .map(Box::new)
                .map(Reply::ExecutionAttempt),
            Action::CancelExecution {
                chat_id,
                generation,
                request_id,
            } => self
                .cancel_execution(chat_id, generation, request_id)
                .await
                .map(Box::new)
                .map(Reply::CancelledExecution),
            Action::CloseExecutionProcess {
                chat_id,
                generation,
            } => self
                .close_execution_process(chat_id, generation)
                .await
                .map(Box::new)
                .map(Reply::ExecutionOwner),
            Action::OpenExecutionOwner {
                operation_id,
                harness,
                title,
            } => self
                .open_execution_owner(operation_id, harness, title)
                .await
                .map(Box::new)
                .map(Reply::ExecutionOwner),
            Action::ReconcileExecutionOwner { input } => self
                .reconcile_execution_owner(input)
                .await
                .map(Box::new)
                .map(Reply::ExecutionOwner),
            Action::StopExecutionOwner { input } => self
                .stop_execution_owner(input)
                .await
                .map(Box::new)
                .map(Reply::ExecutionOwner),
            Action::Open {
                operation_id,
                harness,
                title,
            } => self
                .open(operation_id, harness, title)
                .await
                .map(Box::new)
                .map(Reply::Session),
            Action::Send {
                chat_id,
                generation,
                request_id,
                prompt,
            } => self
                .send(chat_id, generation, request_id, prompt)
                .await
                .map(Box::new)
                .map(Reply::Turn),
            Action::Cancel {
                chat_id,
                generation,
                request_id,
            } => self
                .cancel(chat_id, generation, request_id)
                .await
                .map(Box::new)
                .map(Reply::Turn),
            Action::Rename { input, title } => self
                .context
                .writer()
                .await?
                .rename(input, title)
                .await
                .map(Box::new)
                .map(Reply::Session)
                .map_err(Into::into),
            Action::Session { input, action } => self
                .change(input, action)
                .await
                .map(Box::new)
                .map(Reply::Session),
            Action::LiveText {
                chat_id,
                generation,
                request_id,
            } => {
                let text = self
                    .owners
                    .get(&chat_id)
                    .filter(|owned| owned.binding.generation == generation)
                    .and_then(|owned| owned.sink.live_text(request_id));
                Ok(Reply::PrivateText(text))
            }
            Action::CommandProgress {
                scope,
                chat_id,
                generation,
                request_id,
            } => {
                let value = match self.owners.get(&chat_id) {
                    Some(owned) if owned.binding.generation != generation => {
                        return Err(ChatError::StaleGeneration.into());
                    }
                    Some(owned) => owned.sink.command_progress(scope, request_id)?,
                    None => None,
                };
                Ok(Reply::CommandProgress(value))
            }
        }
    }
    async fn open(
        &mut self,
        operation_id: Uuid,
        harness: ChatHarness,
        title: Option<String>,
    ) -> Result<ChatSessionCommit, ChatServiceError> {
        if operation_id.is_nil() {
            return Err(ChatError::InvalidInput.into());
        }
        // Replays retain the opening HEAD/profile even when the daemon's current
        // route or project HEAD changed. They never launch a replacement owner.
        let writer = self.context.writer().await?;
        if let Some(existing) = writer.session_for_operation(operation_id).await? {
            return writer
                .open(
                    ChatOpen {
                        operation_id,
                        harness,
                        profile_sha256: existing.profile_sha256,
                        resource_key: existing.resource_key,
                        base_commit: existing.base_commit,
                        title_ref: None,
                        timestamp: OffsetDateTime::now_utc(),
                    },
                    title,
                )
                .await
                .map_err(Into::into);
        }
        let prepared = self
            .context
            .dispatch
            .prepare_chat(harness, new_target(harness))
            .await?;
        let root = prepared.launch.working_directory.clone();
        let base_commit = read_head(root).await?;
        let result = writer
            .open(
                ChatOpen {
                    operation_id,
                    harness,
                    profile_sha256: prepared.profile_sha256.clone(),
                    resource_key: prepared.resource_key.clone(),
                    base_commit,
                    title_ref: None,
                    timestamp: OffsetDateTime::now_utc(),
                },
                title,
            )
            .await?;
        if !result.duplicate {
            self.launch(result.session.clone(), prepared).await?;
        }
        Ok(result)
    }
    async fn launch(
        &mut self,
        session: ChatSession,
        prepared: PreparedChat,
    ) -> Result<(), ChatServiceError> {
        if self.owners.contains_key(&session.chat_id) {
            return Err(ChatError::OwnerAlreadyClaimed.into());
        }
        let writer = self.context.writer().await?;
        let claim = writer
            .claim_owner(
                session.chat_id,
                session.generation,
                prepared.protocol.target.expected_id(),
            )
            .await?;
        let binding = RuntimeBinding {
            chat_id: session.chat_id,
            generation: session.generation,
            owner_fence: claim.fence,
        };
        let sink = Arc::new(ChatWriterSink::new(writer.clone(), binding.clone()));
        let root = prepared.launch.working_directory.clone();
        let deadline = prepared.deadline;
        let plan = RuntimePlan {
            binding: binding.clone(),
            launch: prepared.launch,
            protocol: prepared.protocol,
            protocol_limits: SessionLimits::default(),
            limits: RuntimeLimits::default(),
        };
        match spawn_runtime(plan, sink.clone()) {
            Ok(owner) => {
                self.owners.insert(
                    session.chat_id,
                    OwnedRuntime {
                        owner,
                        binding,
                        root,
                        deadline,
                        sink: RuntimeWriterSink::ReadOnly(sink),
                    },
                );
                Ok(())
            }
            Err(error) => {
                // Plan validation failed before spawning any process.
                writer
                    .release_owner(
                        mutation(session.chat_id, session.generation),
                        claim.fence,
                        true,
                        true,
                    )
                    .await?;
                Err(error.into())
            }
        }
    }
    async fn send(
        &mut self,
        id: ChatId,
        generation: u64,
        request_id: Uuid,
        prompt: String,
    ) -> Result<ChatTurnCommit, ChatServiceError> {
        let writer = self.context.writer().await?;
        if let Some(existing) = writer.turn(request_id).await? {
            return writer
                .admit_turn(id, request_id, generation, existing.base_commit, prompt)
                .await
                .map_err(Into::into);
        }
        let session = writer.session(id).await?.ok_or(ChatError::NotFound)?;
        require_generation(&session, generation)?;
        let owned = self.owners.get(&id).ok_or(ChatError::InvalidTransition)?;
        let base = read_head(owned.root.clone()).await?;
        let result = writer
            .admit_turn(id, request_id, generation, base, prompt.clone())
            .await?;
        if !result.duplicate {
            let request = TurnRequest {
                request_id,
                turn_id: *result.turn.turn_id.as_uuid(),
                prompt,
            };
            if let Err(error) = owned.owner.handle.queue_turn(request, owned.deadline) {
                writer
                    .cancel_unsent(request_id, generation, owned.binding.owner_fence)
                    .await?;
                return Err(error.into());
            }
        }
        Ok(result)
    }
    async fn cancel(
        &mut self,
        id: ChatId,
        generation: u64,
        request_id: Uuid,
    ) -> Result<ChatTurnCommit, ChatServiceError> {
        let writer = self.context.writer().await?;
        let turn = writer.turn(request_id).await?.ok_or(ChatError::NotFound)?;
        if turn.chat_id != id || turn.generation != generation {
            return Err(ChatError::Conflict.into());
        }
        let result = writer.cancel_turn(request_id).await?;
        if result.turn.phase == ChatTurnPhase::CancelRequested {
            let owned = self.owners.get(&id).ok_or(ChatError::InvalidTransition)?;
            if owned.binding.generation != generation {
                return Err(ChatError::StaleGeneration.into());
            }
            owned.owner.handle.cancel(*turn.turn_id.as_uuid()).await?;
        }
        Ok(result)
    }
    async fn change(
        &mut self,
        input: ChatMutation,
        action: ChatSessionAction,
    ) -> Result<ChatSessionCommit, ChatServiceError> {
        let writer = self.context.writer().await?.clone();
        let known = writer
            .session_for_operation(input.operation_id)
            .await?
            .is_some();
        if known {
            return apply_session(&writer, input, action).await;
        }
        let session = writer
            .session(input.chat_id)
            .await?
            .ok_or(ChatError::NotFound)?;
        require_generation(&session, input.expected_generation)?;
        if matches!(action, ChatSessionAction::Close) && self.owners.contains_key(&input.chat_id) {
            let accepted = writer.request_close(input.clone()).await?;
            if !accepted.duplicate
                && let Some(owned) = self.owners.remove(&input.chat_id)
            {
                owned.owner.handle.close();
                end_owner(&self.context, owned).await?;
            }
            let mut result = writer.close(input).await?;
            result.duplicate = accepted.duplicate;
            return Ok(result);
        }
        let prepared = if matches!(action, ChatSessionAction::Resume) {
            let target = resume_target(&session)?;
            let prepared = self
                .context
                .dispatch
                .prepare_chat(session.harness, target)
                .await?;
            if prepared.profile_sha256 != session.profile_sha256
                || prepared.resource_key != session.resource_key
            {
                return Err(ChatError::Conflict.into());
            }
            Some(prepared)
        } else {
            None
        };
        let result = apply_session(&writer, input, action).await?;
        if let Some(prepared) = prepared {
            self.launch(result.session.clone(), prepared).await?;
        }
        Ok(result)
    }
}
async fn end_owner(context: &Context, owned: OwnedRuntime) -> Result<(), ChatServiceError> {
    let binding = owned.binding;
    let result = owned.owner.shutdown().await;
    if matches!(result,Ok(ref exit) if exit.writer_confirmed&&exit.cleanup.root_reaped&&exit.cleanup.tree_empty&&exit.cleanup.cleanup_verified)
    {
        return Ok(());
    }
    // A callback failure cannot leave a Ready/Busy record without a process
    // owner. The old fence makes a late notification harmless to replacements.
    context
        .pending
        .lock()
        .map_err(|_| ChatServiceError::Stopped)?
        .insert(binding.chat_id, binding);
    let _ = context.reconcile_pending().await;
    Err(ChatServiceError::AcknowledgementUncertain)
}
async fn apply_session(
    writer: &ChatWriter,
    input: ChatMutation,
    action: ChatSessionAction,
) -> Result<ChatSessionCommit, ChatServiceError> {
    match action {
        ChatSessionAction::Close => writer.close(input).await,
        ChatSessionAction::Archive => writer.archive(input).await,
        ChatSessionAction::Resume => writer.resume(input).await,
        ChatSessionAction::Purge => writer.purge(input).await,
    }
    .map_err(Into::into)
}
fn mutation(chat_id: ChatId, expected_generation: u64) -> ChatMutation {
    ChatMutation {
        operation_id: Uuid::new_v4(),
        chat_id,
        expected_generation,
        timestamp: OffsetDateTime::now_utc(),
    }
}
fn require_generation(session: &ChatSession, generation: u64) -> Result<(), ChatError> {
    if session.generation == generation {
        Ok(())
    } else {
        Err(ChatError::StaleGeneration)
    }
}
fn new_target(harness: ChatHarness) -> SessionTarget {
    match harness {
        ChatHarness::Claude => SessionTarget::Claude {
            session_id: Uuid::new_v4(),
            resume: false,
        },
        ChatHarness::Codex => SessionTarget::Codex {
            resume_thread_id: None,
        },
    }
}
fn resume_target(session: &ChatSession) -> Result<SessionTarget, ChatError> {
    let id = session
        .vendor_session_id
        .clone()
        .ok_or(ChatError::UnconfirmedSession)?;
    match session.harness {
        ChatHarness::Claude => Ok(SessionTarget::Claude {
            session_id: Uuid::parse_str(&id).map_err(|_| ChatError::ProjectionMismatch)?,
            resume: true,
        }),
        ChatHarness::Codex => Ok(SessionTarget::Codex {
            resume_thread_id: Some(id),
        }),
    }
}
async fn read_head(root: PathBuf) -> Result<String, ChatServiceError> {
    tokio::task::spawn_blocking(move || git_head::read_head_commit(&root))
        .await
        .map_err(|_| ChatServiceError::Stopped)?
        .map_err(DispatchServiceError::from)
        .map_err(Into::into)
}

#[cfg(test)]
mod command_progress_tests {
    use super::*;

    #[tokio::test]
    async fn private_progress_read_refuses_the_other_actual_sink_family() {
        let temp = tempfile::tempdir().expect("state");
        let database = temp.path().join("state.sqlite3");
        let worker = tokio::task::spawn_blocking(move || {
            crate::WriterWorker::start_with_config(
                &database,
                crate::DEFAULT_WRITER_QUEUE_CAPACITY,
                Duration::from_secs(30),
            )
        })
        .await
        .expect("writer task")
        .expect("writer");
        let writer = ChatWriter::attach(
            worker.handle().expect("writer handle"),
            temp.path().to_owned(),
        )
        .await
        .expect("private writer");
        let binding = super::super::tests::binding();
        let saved = RuntimeWriterSink::ReadOnly(Arc::new(ChatWriterSink::new(
            writer.clone(),
            binding.clone(),
        )));
        let workspace =
            RuntimeWriterSink::Execution(Arc::new(ChatExecutionSink::new(writer.clone(), binding)));
        let request_id = Uuid::new_v4();
        for (sink, expected, other) in [
            (
                &saved,
                CommandProgressScope::Saved,
                CommandProgressScope::Workspace,
            ),
            (
                &workspace,
                CommandProgressScope::Workspace,
                CommandProgressScope::Saved,
            ),
        ] {
            assert_eq!(
                sink.command_progress(expected, request_id)
                    .expect("valid absence"),
                None
            );
            assert!(matches!(
                sink.command_progress(other, request_id),
                Err(ChatServiceError::Rejected(ChatError::InvalidInput))
            ));
        }
        drop(saved);
        drop(workspace);
        drop(writer);
        tokio::task::spawn_blocking(move || worker.shutdown())
            .await
            .expect("shutdown task")
            .expect("shutdown");
    }
}
