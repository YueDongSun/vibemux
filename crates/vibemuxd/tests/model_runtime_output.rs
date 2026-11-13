use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use tokio::sync::watch;
use uuid::Uuid;
use vibemux_workflow::{
    Sha256Digest, SpecIdentifier,
    gateway::{
        CallOutcome, GatewayProtocol, MAX_COMPLETION_BYTES, ModelCallReceipt, ProviderAttribution,
        ReportedUsage, RouteRole,
    },
    model_call::{ModelCallAdmission, ModelCallError, ModelCallRequest, ModelCallState},
};
use vibemuxd::{
    WriterWorker,
    workflow::model_runtime::{
        ModelCallRuntimeError, ModelGateway, ModelGatewayCompletion, ModelGatewayFailure,
        run_model_call,
    },
};

struct OversizedGateway {
    calls: AtomicUsize,
}

struct CompletedReceiptFailureGateway {
    calls: AtomicUsize,
}

struct CleanupGateway {
    calls: AtomicUsize,
    started: tokio::sync::Notify,
    cleaned: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl ModelGateway for CleanupGateway {
    async fn complete(
        &self,
        _call_id: Uuid,
        _attempt: u32,
        _request: &ModelCallRequest,
        mut cancellation: watch::Receiver<bool>,
    ) -> Result<ModelGatewayCompletion, ModelGatewayFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        while !*cancellation.borrow() {
            cancellation.changed().await.unwrap();
        }
        // An adapter must be allowed to await child cleanup after cancellation.
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        self.cleaned.store(true, Ordering::SeqCst);
        Err(ModelGatewayFailure {
            error_code: "native_call_cancelled",
            receipt: None,
        })
    }
}

async fn cancellation_case(use_deadline: bool) {
    let directory = tempfile::tempdir().unwrap();
    let worker = WriterWorker::start(&directory.path().join("calls.sqlite3")).unwrap();
    let writer = worker.handle().unwrap();
    let gateway = CleanupGateway {
        calls: AtomicUsize::new(0),
        started: tokio::sync::Notify::new(),
        cleaned: std::sync::atomic::AtomicBool::new(false),
    };
    let request = ModelCallRequest {
        role: RouteRole::Compiler,
        requested_alias: "compiler_fixture".into(),
        protocol: GatewayProtocol::NativeCodexExec,
        max_output_tokens: 1024,
        timeout_ms: if use_deadline { 1_000 } else { 10_000 },
        config_sha256: Sha256Digest::of(b"fixture"),
        observation_digest: None,
        system_prompt: "bounded fixture".into(),
        user_prompt: "no real provider called".into(),
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let admission = ModelCallAdmission {
        call_id: Uuid::new_v4(),
        owner_key: SpecIdentifier::new("cleanup_fixture").unwrap(),
        workflow_id: None,
        identity: request.identity().unwrap(),
        deadline_unix_ms: now + 10_000,
        max_calls: 1,
    };
    let (sender, receiver) = watch::channel(false);
    let request_future = run_model_call(
        &writer,
        &gateway,
        request.clone(),
        admission.clone(),
        receiver.clone(),
    );
    let cancel = async {
        gateway.started.notified().await;
        if !use_deadline {
            sender.send(true).unwrap();
        }
    };
    let (result, ()) = tokio::join!(request_future, cancel);
    assert!(matches!(result, Err(ModelCallRuntimeError::OutcomeUnknown)));
    assert!(
        gateway.cleaned.load(Ordering::SeqCst),
        "completion was dropped before its cleanup finished"
    );
    let record = writer.model_call(admission.call_id).unwrap().unwrap();
    assert_eq!(record.state, ModelCallState::Unknown);
    assert_eq!(record.calls_used, 1);
    // Use a fresh, uncancelled receiver. The durable call state must fence replay.
    let (_keep, fresh) = watch::channel(false);
    let replay = run_model_call(&writer, &gateway, request, admission, fresh).await;
    assert!(matches!(replay, Err(ModelCallRuntimeError::NotReplayable)));
    assert_eq!(gateway.calls.load(Ordering::SeqCst), 1);
    worker.shutdown().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_signals_and_joins_gateway_cleanup_before_returning_unknown() {
    cancellation_case(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deadline_signals_and_joins_gateway_cleanup_before_returning_unknown() {
    cancellation_case(true).await;
}

#[async_trait]
impl ModelGateway for OversizedGateway {
    async fn complete(
        &self,
        call_id: Uuid,
        attempt: u32,
        request: &ModelCallRequest,
        _cancellation: watch::Receiver<bool>,
    ) -> Result<ModelGatewayCompletion, ModelGatewayFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let text = "x".repeat(MAX_COMPLETION_BYTES + 1);
        let identity = request.identity().expect("bounded request identity");
        Ok(ModelGatewayCompletion {
            receipt: ModelCallReceipt {
                call_id,
                role: identity.role,
                requested_alias: identity.requested_alias,
                protocol: identity.protocol,
                reported_model: Some("fixture_model".to_string()),
                effective_provider: ProviderAttribution::Unknown,
                usage: ReportedUsage {
                    input_tokens: Some(7),
                    output_tokens: Some(8),
                    cached_tokens: None,
                },
                http_status: Some(200),
                outcome: CallOutcome::Completed,
                failure: None,
                elapsed_ms: 1,
                request_sha256: identity.request_sha256,
                response_sha256: Some(Sha256Digest::of(text.as_bytes())),
                route_generation: identity
                    .observation_digest
                    .expect("pinned fixture route generation"),
                attempt,
            },
            text,
        })
    }
}

#[async_trait]
impl ModelGateway for CompletedReceiptFailureGateway {
    async fn complete(
        &self,
        call_id: Uuid,
        attempt: u32,
        request: &ModelCallRequest,
        _cancellation: watch::Receiver<bool>,
    ) -> Result<ModelGatewayCompletion, ModelGatewayFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let identity = request.identity().expect("bounded request identity");
        Err(ModelGatewayFailure {
            error_code: "gateway_result_invalid",
            receipt: Some(ModelCallReceipt {
                call_id,
                role: identity.role,
                requested_alias: identity.requested_alias,
                protocol: identity.protocol,
                reported_model: Some("fixture_model".to_string()),
                effective_provider: ProviderAttribution::Unknown,
                usage: ReportedUsage::default(),
                http_status: Some(200),
                outcome: CallOutcome::Completed,
                failure: None,
                elapsed_ms: 2,
                request_sha256: identity.request_sha256,
                response_sha256: Some(Sha256Digest::of(b"completed fixture response")),
                route_generation: identity
                    .observation_digest
                    .expect("pinned fixture route generation"),
                attempt,
            }),
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_completed_output_is_rejected_and_never_resubmitted() {
    let directory = tempfile::tempdir().expect("owned database directory");
    let worker = WriterWorker::start(&directory.path().join("model_calls.sqlite3"))
        .expect("single writer starts");
    let writer = worker.handle().expect("writer handle");
    let gateway = OversizedGateway {
        calls: AtomicUsize::new(0),
    };
    let request = ModelCallRequest {
        role: RouteRole::Compiler,
        requested_alias: "compiler_fixture".to_string(),
        protocol: GatewayProtocol::OpenaiChatCompletions,
        max_output_tokens: 1024,
        timeout_ms: 10_000,
        config_sha256: Sha256Digest::of(b"fixture_config"),
        observation_digest: Some(Sha256Digest::of(b"fixture_route")),
        system_prompt: "compile task candidates".to_string(),
        user_prompt: "bounded fixture request".to_string(),
    };
    let call_id = Uuid::new_v4();
    let admission = ModelCallAdmission {
        call_id,
        owner_key: SpecIdentifier::new("oversized_output_fixture").expect("owner key"),
        workflow_id: None,
        identity: request.identity().expect("request identity"),
        deadline_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_millis() as u64
            + 60_000,
        max_calls: 1,
    };
    let (_cancel_sender, cancellation) = watch::channel(false);

    let first = run_model_call(
        &writer,
        &gateway,
        request.clone(),
        admission.clone(),
        cancellation.clone(),
    )
    .await;
    assert!(matches!(
        first,
        Err(ModelCallRuntimeError::ModelCall(
            ModelCallError::ContentTooLarge
        ))
    ));
    let record = writer
        .model_call(call_id)
        .expect("ledger read")
        .expect("persisted call");
    assert_eq!(record.state, ModelCallState::Rejected);
    assert_eq!(record.calls_used, 1);
    assert_eq!(
        record.error_code.as_ref().map(SpecIdentifier::as_str),
        Some("model_call_response_too_large")
    );
    let receipt = record.receipt.expect("actual completed receipt");
    assert_eq!(receipt.outcome, CallOutcome::Completed);
    assert_eq!(receipt.usage.output_tokens, Some(8));
    assert!(receipt.response_sha256.is_some());

    let replay = run_model_call(&writer, &gateway, request, admission, cancellation).await;
    assert!(matches!(replay, Err(ModelCallRuntimeError::NotReplayable)));
    assert_eq!(gateway.calls.load(Ordering::SeqCst), 1);

    worker.shutdown().expect("writer shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completed_receipt_in_gateway_error_is_rejected_persisted_and_never_replayed() {
    let directory = tempfile::tempdir().expect("owned database directory");
    let worker = WriterWorker::start(&directory.path().join("model_calls.sqlite3"))
        .expect("single writer starts");
    let writer = worker.handle().expect("writer handle");
    let gateway = CompletedReceiptFailureGateway {
        calls: AtomicUsize::new(0),
    };
    let request = ModelCallRequest {
        role: RouteRole::Compiler,
        requested_alias: "compiler_fixture".to_string(),
        protocol: GatewayProtocol::OpenaiChatCompletions,
        max_output_tokens: 1024,
        timeout_ms: 10_000,
        config_sha256: Sha256Digest::of(b"fixture_config"),
        observation_digest: Some(Sha256Digest::of(b"fixture_route")),
        system_prompt: "compile task candidates".to_string(),
        user_prompt: "completed receipt failure fixture".to_string(),
    };
    let call_id = Uuid::new_v4();
    let admission = ModelCallAdmission {
        call_id,
        owner_key: SpecIdentifier::new("completed_receipt_failure").expect("owner key"),
        workflow_id: None,
        identity: request.identity().expect("request identity"),
        deadline_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_millis() as u64
            + 60_000,
        max_calls: 1,
    };
    let (_cancel_sender, cancellation) = watch::channel(false);

    let first = run_model_call(
        &writer,
        &gateway,
        request.clone(),
        admission.clone(),
        cancellation.clone(),
    )
    .await;
    assert!(matches!(first, Err(ModelCallRuntimeError::Gateway)));
    let record = writer
        .model_call(call_id)
        .expect("ledger read")
        .expect("persisted rejected call");
    assert_eq!(record.state, ModelCallState::Rejected);
    assert_eq!(record.calls_used, 1);
    assert_eq!(
        record.error_code.as_ref().map(SpecIdentifier::as_str),
        Some("gateway_result_invalid")
    );
    let receipt = record.receipt.expect("preserved completed receipt");
    assert_eq!(receipt.call_id, call_id);
    assert_eq!(receipt.outcome, CallOutcome::Completed);
    assert_eq!(receipt.request_sha256, admission.identity.request_sha256);
    assert!(receipt.response_sha256.is_some());

    let replay = run_model_call(&writer, &gateway, request, admission, cancellation).await;
    assert!(matches!(replay, Err(ModelCallRuntimeError::NotReplayable)));
    assert_eq!(gateway.calls.load(Ordering::SeqCst), 1);

    worker.shutdown().expect("writer shutdown");
}
