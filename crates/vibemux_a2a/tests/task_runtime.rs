use async_trait::async_trait;
use futures::{StreamExt, stream::FuturesUnordered};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::{
    sync::{Semaphore, watch},
    time::{Duration, sleep, timeout},
};
use vibemux_a2a::{
    TaskArtifact, TaskBackend, TaskGatewayError, TaskListRequest, TaskRequest, TaskSnapshot,
    TaskState,
    task_runtime::{TaskExecution, TaskExecutor, TaskRuntime},
};

struct Executor {
    calls: AtomicUsize,
    active: AtomicUsize,
    released: AtomicUsize,
    gate: Semaphore,
}
impl Default for Executor {
    fn default() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            released: AtomicUsize::new(0),
            gate: Semaphore::new(0),
        }
    }
}
struct ActiveGuard(Arc<Executor>);
impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
        self.0.released.fetch_add(1, Ordering::SeqCst);
    }
}
struct TestExecutor(Arc<Executor>);
#[async_trait]
impl TaskExecutor for TestExecutor {
    async fn execute(
        &self,
        _subject: &str,
        request: TaskRequest,
        mut cancellation: watch::Receiver<bool>,
    ) -> Result<TaskExecution, TaskGatewayError> {
        self.0.calls.fetch_add(1, Ordering::SeqCst);
        self.0.active.fetch_add(1, Ordering::SeqCst);
        let _guard = ActiveGuard(self.0.clone());
        let mode = request.payload["mode"].as_str().unwrap_or("complete");
        match mode {
            "cancel_only" => {
                if !*cancellation.borrow() {
                    cancellation
                        .changed()
                        .await
                        .map_err(|_| TaskGatewayError::Server)?;
                }
                Ok(TaskExecution::Canceled)
            }
            "hang" => std::future::pending().await,
            "failure" => Err(TaskGatewayError::Internal),
            "invalid_artifact" => Ok(TaskExecution::Completed(vec![TaskArtifact::data(
                "bad",
                json!({"text":"x".repeat(20000)}),
            )])),
            _ => {
                self.0
                    .gate
                    .acquire()
                    .await
                    .map_err(|_| TaskGatewayError::Internal)?
                    .forget();
                assert_ne!(mode, "panic", "controlled executor panic");
                Ok(TaskExecution::Completed(vec![TaskArtifact::data(
                    "result",
                    json!({"verified":true}),
                )]))
            }
        }
    }
}
fn start(parallel: usize) -> (TaskRuntime, Arc<Executor>) {
    let state = Arc::new(Executor::default());
    let runtime =
        TaskRuntime::start(Arc::new(TestExecutor(state.clone())), parallel).expect("start");
    (runtime, state)
}
fn request(id: &str, mode: &str) -> TaskRequest {
    TaskRequest {
        request_id: id.to_string(),
        context_id: format!("context_{id}"),
        task_id: None,
        idempotency_key: format!("key_{id}"),
        payload: json!({"mode":mode}),
    }
}
async fn wait_calls(executor: &Executor, count: usize) {
    timeout(Duration::from_secs(2), async {
        while executor.calls.load(Ordering::SeqCst) < count {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("executor starts");
}
async fn wait_terminal(backend: &dyn TaskBackend, subject: &str, id: &str) -> TaskSnapshot {
    timeout(Duration::from_secs(2), async {
        loop {
            let task = backend.get(subject, id).await.expect("get");
            if task.state.is_terminal() {
                return task;
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("terminal")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_idempotency_executes_once_and_conflicting_payload_is_rejected() {
    let (runtime, executor) = start(2);
    let backend = runtime.backend();
    let original = request("same", "complete");
    let sends = (0..16).map(|sequence| {
        let backend = backend.clone();
        let mut request = original.clone();
        request.request_id = format!("attempt_{sequence}");
        async move {
            backend
                .send("alice", request)
                .await
                .expect("duplicate accepted")
        }
    });
    let results = futures::future::join_all(sends).await;
    let task_id = results[0].task_id.clone();
    assert!(results.iter().all(|task| task.task_id == task_id));
    wait_calls(&executor, 1).await;
    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
    let mut conflicting = original.clone();
    conflicting.payload = json!({"mode":"different"});
    assert_eq!(
        backend
            .send("alice", conflicting)
            .await
            .expect_err("conflict"),
        TaskGatewayError::InvalidRequest
    );
    let bob = backend
        .send("bob", original)
        .await
        .expect("subject-key independent");
    assert_ne!(bob.task_id, task_id);
    wait_calls(&executor, 2).await;
    executor.gate.add_permits(2);
    assert_eq!(
        wait_terminal(backend.as_ref(), "alice", &task_id)
            .await
            .state,
        TaskState::Completed
    );
    assert_eq!(
        wait_terminal(backend.as_ref(), "bob", &bob.task_id)
            .await
            .state,
        TaskState::Completed
    );
    runtime.shutdown().await.expect("joined");
    assert_eq!(executor.active.load(Ordering::SeqCst), 0);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authorization_list_pagination_and_parallel_capacity_are_enforced() {
    let (runtime, executor) = start(4);
    let backend = runtime.backend();
    let mut ids = vec![];
    for index in 0..3 {
        ids.push(
            backend
                .send("alice", request(&format!("a_{index}"), "complete"))
                .await
                .expect("task")
                .task_id,
        );
    }
    let bob = backend
        .send("bob", request("bob", "complete"))
        .await
        .expect("bob");
    wait_calls(&executor, 4).await;
    assert_eq!(
        backend
            .send("alice", request("over_capacity", "complete"))
            .await
            .expect_err("parallel bound"),
        TaskGatewayError::Busy
    );
    assert_eq!(
        backend
            .get("alice", &bob.task_id)
            .await
            .expect_err("get denied"),
        TaskGatewayError::NotFound
    );
    assert_eq!(
        backend
            .cancel("alice", &bob.task_id)
            .await
            .expect_err("cancel denied"),
        TaskGatewayError::NotFound
    );
    assert!(matches!(
        backend.subscribe("alice", &bob.task_id).await,
        Err(TaskGatewayError::NotFound)
    ));
    assert_eq!(
        backend
            .get("", &bob.task_id)
            .await
            .expect_err("invalid subject"),
        TaskGatewayError::InvalidRequest
    );
    executor.gate.add_permits(4);
    for id in &ids {
        wait_terminal(backend.as_ref(), "alice", id).await;
    }
    wait_terminal(backend.as_ref(), "bob", &bob.task_id).await;
    let mut query = TaskListRequest {
        context_id: None,
        state: Some(TaskState::Completed),
        page_size: 1,
        page_token: None,
        include_artifacts: false,
    };
    let mut seen = vec![];
    loop {
        let page = backend.list("alice", query.clone()).await.expect("page");
        assert_eq!(page.total_size, 3);
        assert_eq!(page.tasks.len(), 1);
        assert!(page.tasks[0].artifacts.is_empty());
        seen.push(page.tasks[0].task_id.clone());
        if page.next_page_token.is_none() {
            break;
        }
        query.page_token = page.next_page_token;
    }
    seen.sort();
    ids.sort();
    assert_eq!(seen, ids);
    query.page_token = Some(usize::MAX.to_string());
    assert_eq!(
        backend
            .list("alice", query)
            .await
            .expect_err("invalid offset"),
        TaskGatewayError::InvalidRequest
    );
    runtime.shutdown().await.expect("shutdown");
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_waiters_are_bounded_pruned_and_late_cancel_preserves_completion() {
    let (runtime, executor) = start(1);
    let backend = runtime.backend();
    let task = backend
        .send("alice", request("late", "complete"))
        .await
        .expect("task");
    wait_calls(&executor, 1).await;
    let mut cancels = FuturesUnordered::new();
    for _ in 0..9 {
        let backend = backend.clone();
        let id = task.task_id.clone();
        cancels.push(async move { backend.cancel("alice", &id).await });
    }
    assert_eq!(
        timeout(Duration::from_secs(2), cancels.next())
            .await
            .expect("capacity reply")
            .expect("result")
            .expect_err("ninth waiter"),
        TaskGatewayError::Busy
    );
    drop(cancels);
    let next = backend.cancel("alice", &task.task_id);
    tokio::pin!(next);
    assert!(futures::poll!(&mut next).is_pending());
    backend
        .get("alice", &task.task_id)
        .await
        .expect("actor FIFO barrier");
    assert!(futures::poll!(&mut next).is_pending());
    executor.gate.add_permits(1);
    let completed = next.await.expect("closed waiter slots reclaimed");
    assert_eq!(completed.state, TaskState::Completed);
    assert_eq!(completed.artifacts.len(), 1);
    assert_eq!(
        backend
            .cancel("alice", &task.task_id)
            .await
            .expect_err("already complete"),
        TaskGatewayError::Unsupported
    );
    runtime.shutdown().await.expect("shutdown");
    assert_eq!(executor.active.load(Ordering::SeqCst), 0);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subscriptions_have_initial_terminal_snapshots_and_a_reader_bound() {
    let (runtime, executor) = start(1);
    let backend = runtime.backend();
    let task = backend
        .send("alice", request("subscribe", "cancel_only"))
        .await
        .expect("task");
    wait_calls(&executor, 1).await;
    let mut readers = vec![];
    for _ in 0..32 {
        readers.push(
            backend
                .subscribe("alice", &task.task_id)
                .await
                .expect("reader"),
        );
    }
    assert!(matches!(
        backend.subscribe("alice", &task.task_id).await,
        Err(TaskGatewayError::Busy)
    ));
    let mut reader = readers.pop().expect("reader");
    drop(readers);
    assert_eq!(
        reader
            .next()
            .await
            .expect("initial")
            .expect("snapshot")
            .state,
        TaskState::Working
    );
    assert_eq!(
        backend
            .cancel("alice", &task.task_id)
            .await
            .expect("cancel")
            .state,
        TaskState::Canceled
    );
    assert_eq!(
        reader
            .next()
            .await
            .expect("terminal")
            .expect("snapshot")
            .state,
        TaskState::Canceled
    );
    assert!(reader.next().await.is_none());
    let mut late = backend
        .subscribe("alice", &task.task_id)
        .await
        .expect("internal terminal subscription");
    assert_eq!(
        late.next()
            .await
            .expect("initial terminal")
            .expect("snapshot")
            .state,
        TaskState::Canceled
    );
    assert!(late.next().await.is_none());
    runtime.shutdown().await.expect("shutdown");
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn panic_is_bound_to_its_task_and_shutdown_joins_the_other_job() {
    let (runtime, executor) = start(2);
    let backend = runtime.backend();
    let sibling = backend
        .send("alice", request("sibling", "cancel_only"))
        .await
        .expect("sibling");
    let panicking = backend
        .send("alice", request("panic", "panic"))
        .await
        .expect("panicking");
    wait_calls(&executor, 2).await;
    let mut sibling_events = backend
        .subscribe("alice", &sibling.task_id)
        .await
        .expect("subscribe");
    sibling_events
        .next()
        .await
        .expect("initial")
        .expect("snapshot");
    executor.gate.add_permits(1);
    let failed = wait_terminal(backend.as_ref(), "alice", &panicking.task_id).await;
    assert_eq!(failed.state, TaskState::Failed);
    assert_eq!(failed.error_code.as_deref(), Some("a2a_task_internal"));
    assert_eq!(
        backend
            .get("alice", &sibling.task_id)
            .await
            .expect("owner remains available")
            .state,
        TaskState::Working
    );
    assert_eq!(
        runtime
            .shutdown()
            .await
            .expect_err("panic evidence preserved"),
        TaskGatewayError::Internal
    );
    assert_eq!(executor.active.load(Ordering::SeqCst), 0);
    assert_eq!(executor.released.load(Ordering::SeqCst), 2);
    assert_eq!(
        sibling_events
            .next()
            .await
            .expect("sibling terminal")
            .expect("snapshot")
            .state,
        TaskState::Canceled
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_and_invalid_executor_results_never_become_completed() {
    let (runtime, executor) = start(2);
    let backend = runtime.backend();
    for mode in ["failure", "invalid_artifact"] {
        let task = backend
            .send("alice", request(mode, mode))
            .await
            .expect("task");
        let failed = wait_terminal(backend.as_ref(), "alice", &task.task_id).await;
        assert_eq!(failed.state, TaskState::Failed);
        assert!(failed.artifacts.is_empty());
        assert!(failed.error_code.is_some());
    }
    runtime.shutdown().await.expect("clean owner shutdown");
    assert_eq!(executor.active.load(Ordering::SeqCst), 0);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_deadline_aborts_joins_and_publishes_failure() {
    let (runtime, executor) = start(1);
    let backend = runtime.backend();
    let task = backend
        .send("alice", request("hang", "hang"))
        .await
        .expect("task");
    wait_calls(&executor, 1).await;
    let mut events = backend
        .subscribe("alice", &task.task_id)
        .await
        .expect("subscribe");
    events.next().await.expect("initial").expect("snapshot");
    assert_eq!(
        timeout(Duration::from_secs(12), runtime.shutdown())
            .await
            .expect("bounded shutdown")
            .expect_err("forced cleanup"),
        TaskGatewayError::Deadline
    );
    assert_eq!(executor.active.load(Ordering::SeqCst), 0);
    let failed = events
        .next()
        .await
        .expect("terminal failure")
        .expect("snapshot");
    assert_eq!(failed.state, TaskState::Failed);
    assert_eq!(failed.error_code.as_deref(), Some("a2a_task_deadline"));
    assert!(events.next().await.is_none());
    assert_eq!(
        backend
            .get("alice", &task.task_id)
            .await
            .expect_err("owner closed"),
        TaskGatewayError::Server
    );
}
#[test]
fn start_without_async_runtime_is_an_error_not_a_panic() {
    let executor = Arc::new(TestExecutor(Arc::new(Executor::default())));
    assert!(matches!(
        TaskRuntime::start(executor, 1),
        Err(TaskGatewayError::Server)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retained_task_limit_does_not_reset_idempotency_or_allocate_more_jobs() {
    let (runtime, executor) = start(1);
    let backend = runtime.backend();
    let first_request = request("retained_0", "failure");
    let mut first_id = None;
    for index in 0..64 {
        let request = if index == 0 {
            first_request.clone()
        } else {
            request(&format!("retained_{index}"), "failure")
        };
        let task = backend
            .send("alice", request)
            .await
            .expect("bounded retained task");
        wait_terminal(backend.as_ref(), "alice", &task.task_id).await;
        if index == 0 {
            first_id = Some(task.task_id);
        }
    }
    assert_eq!(
        backend
            .send("alice", request("retained_overflow", "failure"))
            .await
            .expect_err("retained limit"),
        TaskGatewayError::Busy
    );
    assert_eq!(
        Some(
            backend
                .send("alice", first_request)
                .await
                .expect("existing key remains valid")
                .task_id
        ),
        first_id
    );
    assert_eq!(executor.calls.load(Ordering::SeqCst), 64);
    runtime.shutdown().await.expect("shutdown");
    assert_eq!(executor.active.load(Ordering::SeqCst), 0);
}

#[derive(Default)]
struct NumericExecutor {
    calls: AtomicUsize,
    inputs: std::sync::Mutex<Vec<serde_json::Value>>,
}
#[async_trait]
impl TaskExecutor for NumericExecutor {
    async fn execute(
        &self,
        _subject: &str,
        request: TaskRequest,
        _cancellation: watch::Receiver<bool>,
    ) -> Result<TaskExecution, TaskGatewayError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inputs
            .lock()
            .expect("captured inputs")
            .push(request.payload);
        Ok(TaskExecution::Completed(vec![TaskArtifact::data(
            "numeric_result",
            json!({"ok":true}),
        )]))
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn numeric_identity_retries_through_http_and_jsonrpc_execute_once() {
    use vibemux_a2a::{
        PeerCredential, TaskBinding, TaskClient, TaskClientConfig, TaskServer, TaskServerConfig,
    };
    let executor = Arc::new(NumericExecutor::default());
    let runtime = TaskRuntime::start(executor.clone(), 1).expect("runtime");
    let backend = runtime.backend();
    let credential = PeerCredential {
        subject: "alice".into(),
        bearer_token: "numeric_identity_test_token_abcdefghijklmnopqrstuvwxyz".into(),
    };
    let server = TaskServer::start(
        TaskServerConfig {
            peer_id: "numeric_peer".into(),
            credentials: vec![credential.clone()],
        },
        backend.clone(),
    )
    .await
    .expect("server");
    let mut original = request("numeric", "complete");
    original.payload = json!({"value":1,"nested":[-2,{"zero":0,"fraction":1.5}],"text":"1"});
    // Seed via the typed backend so the first identity retains JSON integers;
    // real wire retries cross the SDK's Protobuf Struct numeric conversion.
    let initial = backend
        .send("alice", original.clone())
        .await
        .expect("initial integer request");
    wait_terminal(backend.as_ref(), "alice", &initial.task_id).await;
    for binding in [TaskBinding::JsonRpc, TaskBinding::HttpJson] {
        let client = TaskClient::connect(
            server.base_url(),
            TaskClientConfig {
                allowed_origins: vec![server.base_url().into()],
                credential: credential.clone(),
                preferred_bindings: vec![binding],
            },
        )
        .await
        .expect("real client");
        let mut retry = original.clone();
        retry.request_id = format!("retry_{binding:?}");
        retry.payload = if binding == TaskBinding::JsonRpc {
            original.payload.clone()
        } else {
            json!({"value":1.0,"nested":[-2.0,{"fraction":1.5,"zero":-0.0}],"text":"1"})
        };
        let repeated = client.send(&retry).await.expect("equivalent numeric retry");
        assert_eq!(repeated.task_id, initial.task_id);
        retry.payload["value"] = json!(1.5);
        assert_eq!(
            client
                .send(&retry)
                .await
                .expect_err("different number conflicts"),
            TaskGatewayError::InvalidRequest
        );
        let mut unsafe_request = request(&format!("unsafe_{binding:?}"), "complete");
        unsafe_request.payload = json!({"value":9_007_199_254_740_992_u64});
        assert_eq!(
            client
                .send(&unsafe_request)
                .await
                .expect_err("unsafe integer rejected"),
            TaskGatewayError::InvalidRequest
        );
    }
    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
    assert!(
        executor.inputs.lock().expect("inputs")[0]["value"].is_i64(),
        "fingerprinting must not rewrite executor input"
    );
    server.shutdown().await.expect("server shutdown");
    runtime.shutdown().await.expect("runtime shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn numeric_identity_safe_integer_limits_remain_exact() {
    let executor = Arc::new(NumericExecutor::default());
    let runtime = TaskRuntime::start(executor.clone(), 1).expect("runtime");
    let backend = runtime.backend();
    for (id, integer, float) in [
        (
            "positive_limit",
            9_007_199_254_740_991_i64,
            9_007_199_254_740_991.0_f64,
        ),
        (
            "negative_limit",
            -9_007_199_254_740_991_i64,
            -9_007_199_254_740_991.0_f64,
        ),
    ] {
        let mut original = request(id, "complete");
        original.payload = json!({"value":integer});
        let first = backend
            .send("alice", original.clone())
            .await
            .expect("safe integer");
        wait_terminal(backend.as_ref(), "alice", &first.task_id).await;
        original.request_id.push_str("_retry");
        original.payload = json!({"value":float});
        assert_eq!(
            backend
                .send("alice", original)
                .await
                .expect("exact floating representation")
                .task_id,
            first.task_id
        );
    }
    assert_eq!(executor.calls.load(Ordering::SeqCst), 2);
    runtime.shutdown().await.expect("shutdown");
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsafe_integral_payloads_are_rejected_before_task_creation_or_execution() {
    let executor = Arc::new(NumericExecutor::default());
    let runtime = TaskRuntime::start(executor.clone(), 1).expect("runtime");
    let backend = runtime.backend();
    for (index, value) in [
        json!(9_007_199_254_740_992_u64),
        json!(-9_007_199_254_740_992_i64),
        json!(9_007_199_254_740_992.0_f64),
        json!(-9_007_199_254_740_992.0_f64),
        json!(u64::MAX),
        json!(i64::MIN),
    ]
    .into_iter()
    .enumerate()
    {
        let mut invalid = request(&format!("unsafe_integer_{index}"), "complete");
        invalid.payload = json!({"nested":[{"value":value}]});
        assert_eq!(
            backend
                .send("alice", invalid)
                .await
                .expect_err("outside exact integer domain"),
            TaskGatewayError::InvalidRequest
        );
    }
    assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
    assert!(executor.inputs.lock().expect("inputs").is_empty());
    let page = backend
        .list(
            "alice",
            TaskListRequest {
                context_id: None,
                state: None,
                page_size: 32,
                page_token: None,
                include_artifacts: false,
            },
        )
        .await
        .expect("list");
    assert_eq!(page.total_size, 0);
    runtime.shutdown().await.expect("shutdown");
}
