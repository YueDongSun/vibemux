use super::*;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum GateStage {
    Admission,
    Decision,
    Completion,
    RejectDispatch,
}
pub(super) struct Gate {
    stage: GateStage,
    entered: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
    used: std::sync::atomic::AtomicBool,
}
impl Gate {
    pub(super) fn new(stage: GateStage) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            stage,
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Semaphore::new(0),
            used: std::sync::atomic::AtomicBool::new(false),
        })
    }
    async fn hold(&self, stage: GateStage) {
        if self.stage == stage && !self.used.swap(true, std::sync::atomic::Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.acquire().await.expect("gate open").forget();
        }
    }
    pub(super) async fn observed(&self) {
        timeout(WAIT, self.entered.notified())
            .await
            .expect("specific owned callback entered");
    }
    pub(super) fn open(&self) {
        self.release.add_permits(1);
    }
}
pub(super) struct GatedSink {
    pub(super) inner: std::sync::Arc<dyn vibemuxd::chat_session::ChatRuntimeSink>,
    pub(super) gate: std::sync::Arc<Gate>,
}
#[async_trait::async_trait]
impl vibemuxd::chat_session::ChatRuntimeSink for GatedSink {
    async fn validate_approval_dispatch(
        &self,
        b: &RuntimeBinding,
        e: &vibemuxd::chat_writer::ChatApprovalEvidence,
    ) -> Result<(), vibemuxd::chat_session::RuntimeError> {
        self.inner.validate_approval_dispatch(b, e).await
    }
    async fn claim_turn(
        &self,
        b: &RuntimeBinding,
        t: &TurnRequest,
    ) -> Result<(), vibemuxd::chat_session::RuntimeError> {
        self.inner.claim_turn(b, t).await
    }
    async fn event(
        &self,
        b: &RuntimeBinding,
        e: vibemuxd::chat_session::RuntimeEvent,
    ) -> Result<(), vibemuxd::chat_session::RuntimeError> {
        self.inner.event(b, e).await
    }
    async fn cancel_turn(
        &self,
        b: &RuntimeBinding,
        t: vibemux_harness::chat_session::TurnSummary,
    ) -> Result<(), vibemuxd::chat_session::RuntimeError> {
        self.inner.cancel_turn(b, t).await
    }
    async fn admit_approval(
        &self,
        b: &RuntimeBinding,
        c: vibemux_harness::chat_session::ApprovalChallenge,
        expires: OffsetDateTime,
    ) -> Result<ChatApproval, vibemuxd::chat_session::RuntimeError> {
        let result = self.inner.admit_approval(b, c, expires).await?;
        self.gate.hold(GateStage::Admission).await;
        Ok(result)
    }
    async fn decide_approval(
        &self,
        b: &RuntimeBinding,
        r: RuntimeApprovalDecision,
    ) -> Result<
        vibemuxd::chat_writer::ChatApprovalDecisionResult,
        vibemuxd::chat_session::RuntimeError,
    > {
        let mut result = self.inner.decide_approval(b, r).await?;
        if self.gate.stage == GateStage::RejectDispatch
            && let Some(dispatch) = result.dispatch.take()
        {
            let (claim, _) = dispatch.into_parts();
            result.receipt.approval = self
                .inner
                .complete_approval(b, claim, ChatApprovalDelivery::Unknown)
                .await?;
        }
        self.gate.hold(GateStage::Decision).await;
        Ok(result)
    }
    async fn complete_approval(
        &self,
        b: &RuntimeBinding,
        c: vibemux_store::ChatApprovalDeliveryClaim,
        o: ChatApprovalDelivery,
    ) -> Result<ChatApproval, vibemuxd::chat_session::RuntimeError> {
        self.gate.hold(GateStage::Completion).await;
        self.inner.complete_approval(b, c, o).await
    }
    async fn expire_approval(
        &self,
        b: &RuntimeBinding,
        id: vibemux_types::ChatApprovalId,
    ) -> Result<ChatApproval, vibemuxd::chat_session::RuntimeError> {
        self.inner.expire_approval(b, id).await
    }
}
impl Fixture {
    async fn runtime_with_gate(&self, gate: std::sync::Arc<Gate>) -> RuntimeOwner {
        vibemuxd::chat_session::fixture_runtime::spawn_fixture_approval_runtime_with_sink(
            self.chats.clone(),
            self.binding.clone(),
            FixtureApprovalSettings {
                project_root: self.project.clone(),
                git: self.git.clone(),
                vendor_directory: self.vendor.clone(),
                trampoline: PathBuf::from(env!("CARGO_BIN_EXE_vibemux_launch_trampoline")),
                runtime_limits: RuntimeLimits::default(),
            },
            self.config.clone(),
            move |inner| std::sync::Arc::new(GatedSink { inner, gate }),
        )
        .await
        .expect("guarded gated runtime")
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn output_and_cancellation_continue_while_a_committed_admission_result_is_parked() {
    let fixture = Fixture::new().await;
    let gate = Gate::new(GateStage::Admission);
    let runtime = fixture.runtime_with_gate(gate.clone()).await;
    let request = fixture.send(&runtime).await;
    gate.observed().await;
    assert!(
        runtime.handle.pending_approval().is_none(),
        "no human offer before the admission callback returns"
    );
    timeout(WAIT, async {
        while runtime.handle.snapshot().records_received < 5 {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("extra output consumed while callback parked");
    let attempt = fixture
        .chats
        .execution_attempt(request)
        .await
        .expect("attempt")
        .expect("known");
    timeout(
        Duration::from_secs(2),
        runtime.handle.cancel(*attempt.attempt_id.as_uuid()),
    )
    .await
    .expect("cancel remained responsive")
    .expect("cancel accepted");
    assert!(
        !PathBuf::from(&fixture.workspace.path)
            .join(FIXTURE_APPROVAL_FILE)
            .exists()
    );
    gate.open();
    fixture.cleanup(runtime).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_committed_allow_returning_after_cancel_never_enters_stdin() {
    let fixture = Fixture::new().await;
    let gate = Gate::new(GateStage::Decision);
    let runtime = fixture.runtime_with_gate(gate.clone()).await;
    let request = fixture.send(&runtime).await;
    let offered = challenge(&runtime).await;
    let handle = runtime.handle.clone();
    let choose = RuntimeApprovalDecision {
        approval_id: offered.approval_id,
        operation_id: Uuid::new_v4(),
        expected_version: offered.version,
        decision: ChatApprovalDecision::AllowOnce,
    };
    let decision = tokio::spawn(async move { handle.decide_approval(choose).await });
    gate.observed().await;
    let attempt = fixture
        .chats
        .execution_attempt(request)
        .await
        .expect("attempt")
        .expect("known");
    runtime
        .handle
        .cancel(*attempt.attempt_id.as_uuid())
        .await
        .expect("cancel after claim");
    gate.open();
    let _ = decision.await.expect("decision observer");
    timeout(WAIT, async {
        loop {
            let record = fixture
                .chats
                .approval_metadata(fixture.owner.chat_id, offered.approval_id)
                .await
                .expect("record");
            if record.delivery == Some(ChatApprovalDelivery::Unknown) {
                assert_eq!(record.decision, Some(ChatApprovalDecision::AllowOnce));
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("unknown delivery sealed");
    assert!(
        !PathBuf::from(&fixture.workspace.path)
            .join(FIXTURE_APPROVAL_FILE)
            .exists()
    );
    fixture.cleanup(runtime).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn terminal_waits_for_the_durable_response_write_receipt() {
    let fixture = Fixture::new().await;
    let gate = Gate::new(GateStage::Completion);
    let runtime = fixture.runtime_with_gate(gate.clone()).await;
    let request = fixture.send(&runtime).await;
    let offered = challenge(&runtime).await;
    runtime
        .handle
        .decide_approval(RuntimeApprovalDecision {
            approval_id: offered.approval_id,
            operation_id: Uuid::new_v4(),
            expected_version: offered.version,
            decision: ChatApprovalDecision::AllowOnce,
        })
        .await
        .expect("allow");
    gate.observed().await;
    let effect = PathBuf::from(&fixture.workspace.path).join(FIXTURE_APPROVAL_FILE);
    timeout(WAIT, async {
        while !effect.exists() || runtime.handle.snapshot().records_received < 6 {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("vendor effect and terminal observed");
    assert!(
        runtime.handle.snapshot().active_turn.is_some(),
        "terminal is buffered"
    );
    let before = fixture
        .chats
        .execution_attempt(request)
        .await
        .expect("attempt")
        .expect("known");
    assert!(!before.phase.is_terminal());
    assert_eq!(
        fixture
            .chats
            .approval_metadata(fixture.owner.chat_id, offered.approval_id)
            .await
            .expect("receipt")
            .delivery,
        Some(ChatApprovalDelivery::Claimed)
    );
    gate.open();
    timeout(WAIT, async {
        loop {
            if fixture
                .chats
                .execution_attempt(request)
                .await
                .expect("attempt")
                .expect("known")
                .phase
                .is_terminal()
            {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("finish after durable receipt");
    let events = fixture.cleanup(runtime).await;
    let delivered = events
        .iter()
        .position(|e| e.event_type().as_str() == "chat_approval_delivery_observed")
        .expect("delivery event");
    let finished = events
        .iter()
        .position(|e| e.event_type().as_str() == "chat_execution_attempt_finished")
        .expect("finish event");
    assert!(delivered < finished);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_head_change_after_the_claim_refuses_the_prepared_affirmative_frame() {
    let fixture = Fixture::new().await;
    let gate = Gate::new(GateStage::Decision);
    let runtime = fixture.runtime_with_gate(gate.clone()).await;
    fixture.send(&runtime).await;
    let offered = challenge(&runtime).await;
    let handle = runtime.handle.clone();
    let decision = tokio::spawn(async move {
        handle
            .decide_approval(RuntimeApprovalDecision {
                approval_id: offered.approval_id,
                operation_id: Uuid::new_v4(),
                expected_version: offered.version,
                decision: ChatApprovalDecision::AllowOnce,
            })
            .await
    });
    gate.observed().await;
    let run = PathBuf::from(&fixture.workspace.path);
    std::fs::write(run.join("base.txt"), "advanced owned branch\n").expect("owned branch edit");
    git(&fixture.git, &run, &["add", "base.txt"]).await;
    git(
        &fixture.git,
        &run,
        &["commit", "--quiet", "-m", "advance owned branch"],
    )
    .await;
    gate.open();
    let _ = decision.await.expect("decision task");
    timeout(WAIT, async {
        loop {
            let record = fixture
                .chats
                .approval_metadata(fixture.owner.chat_id, offered.approval_id)
                .await
                .expect("record");
            if record.delivery == Some(ChatApprovalDelivery::Unknown) {
                assert_eq!(record.decision, Some(ChatApprovalDecision::AllowOnce));
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("changed HEAD sealed unknown");
    assert!(!run.join(FIXTURE_APPROVAL_FILE).exists());
    fixture.cleanup(runtime).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_dispatch_retires_the_stale_pending_offer_and_closes_promptly() {
    let fixture = Fixture::new().await;
    let gate = Gate::new(GateStage::RejectDispatch);
    let runtime = fixture.runtime_with_gate(gate).await;
    let request = fixture.send(&runtime).await;
    let offered = challenge(&runtime).await;
    let receipt = runtime
        .handle
        .decide_approval(RuntimeApprovalDecision {
            approval_id: offered.approval_id,
            operation_id: Uuid::new_v4(),
            expected_version: offered.version,
            decision: ChatApprovalDecision::AllowOnce,
        })
        .await
        .expect("retained decision receipt");
    assert_eq!(receipt.delivery, Some(ChatApprovalDelivery::Unknown));
    timeout(Duration::from_secs(4), async {
        loop {
            if runtime
                .handle
                .pending_approval()
                .is_none_or(|v| v.state != vibemux_types::chat_approval::ChatApprovalState::Pending)
                && fixture
                    .chats
                    .execution_attempt(request)
                    .await
                    .expect("attempt")
                    .expect("known")
                    .phase
                    .is_terminal()
            {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("no stale Pending/long human wait");
    assert!(
        !PathBuf::from(&fixture.workspace.path)
            .join(FIXTURE_APPROVAL_FILE)
            .exists()
    );
    fixture.cleanup(runtime).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn losing_the_client_decision_observer_does_not_abandon_the_owned_delivery() {
    let fixture = Fixture::new().await;
    let gate = Gate::new(GateStage::Decision);
    let runtime = fixture.runtime_with_gate(gate.clone()).await;
    let request = fixture.send(&runtime).await;
    let offered = challenge(&runtime).await;
    let handle = runtime.handle.clone();
    let task = tokio::spawn(async move {
        handle
            .decide_approval(RuntimeApprovalDecision {
                approval_id: offered.approval_id,
                operation_id: Uuid::new_v4(),
                expected_version: offered.version,
                decision: ChatApprovalDecision::AllowOnce,
            })
            .await
    });
    gate.observed().await;
    task.abort();
    assert!(
        task.await
            .expect_err("client observer cancelled")
            .is_cancelled()
    );
    gate.open();
    timeout(WAIT, async {
        loop {
            let attempt = fixture
                .chats
                .execution_attempt(request)
                .await
                .expect("attempt")
                .expect("known");
            if attempt.phase.is_terminal() {
                assert_eq!(attempt.outcome, Some(ChatTurnOutcome::Completed));
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("owned continuation");
    assert_eq!(
        std::fs::read(PathBuf::from(&fixture.workspace.path).join(FIXTURE_APPROVAL_FILE))
            .expect("single authorized effect"),
        FIXTURE_APPROVAL_CONTENT.as_bytes()
    );
    fixture.cleanup(runtime).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_human_wait_pauses_idle_but_expires_with_the_original_turn_budget() {
    let fixture = Fixture::new().await;
    let limits = RuntimeLimits {
        idle: Duration::from_secs(2),
        ..RuntimeLimits::default()
    };
    let runtime = fixture.runtime(limits).await;
    let request = fixture
        .send_with_deadline(&runtime, Duration::from_secs(6))
        .await;
    let offered = challenge(&runtime).await;
    sleep(Duration::from_secs(3)).await;
    assert!(
        runtime.handle.snapshot().active_turn.is_some(),
        "human wait survives output idle"
    );
    assert_eq!(
        fixture
            .chats
            .approval_metadata(fixture.owner.chat_id, offered.approval_id)
            .await
            .expect("pending")
            .state,
        vibemux_types::chat_approval::ChatApprovalState::Pending
    );
    timeout(WAIT, async {
        loop {
            let record = fixture
                .chats
                .approval_metadata(fixture.owner.chat_id, offered.approval_id)
                .await
                .expect("expiry");
            if record.state == vibemux_types::chat_approval::ChatApprovalState::Expired {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("total turn expiry");
    assert!(
        runtime
            .handle
            .decide_approval(RuntimeApprovalDecision {
                approval_id: offered.approval_id,
                operation_id: Uuid::new_v4(),
                expected_version: offered.version,
                decision: ChatApprovalDecision::AllowOnce
            })
            .await
            .is_err()
    );
    assert!(
        !PathBuf::from(&fixture.workspace.path)
            .join(FIXTURE_APPROVAL_FILE)
            .exists()
    );
    timeout(WAIT, async {
        while !fixture
            .chats
            .execution_attempt(request)
            .await
            .expect("attempt")
            .expect("known")
            .phase
            .is_terminal()
        {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("expiry settles attempt");
    fixture.cleanup(runtime).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mismatched_vendor_or_trampoline_is_refused_before_the_fixture_launch() {
    for wrong_trampoline in [false, true] {
        let fixture = Fixture::new().await;
        let trampoline = if wrong_trampoline {
            let path = fixture.vendor.join("wrong_trampoline.exe");
            std::fs::copy(env!("CARGO_BIN_EXE_vibemux_launch_trampoline"), &path)
                .expect("owned wrong trampoline copy");
            path
        } else {
            std::fs::copy(
                env!("CARGO_BIN_EXE_vibemux_launch_trampoline"),
                fixture.vendor.join("claude.exe"),
            )
            .expect("mismatched alias");
            PathBuf::from(env!("CARGO_BIN_EXE_vibemux_launch_trampoline"))
        };
        let result = spawn_fixture_approval_runtime(
            fixture.chats.clone(),
            fixture.binding.clone(),
            FixtureApprovalSettings {
                project_root: fixture.project.clone(),
                git: fixture.git.clone(),
                vendor_directory: fixture.vendor.clone(),
                trampoline,
                runtime_limits: RuntimeLimits::default(),
            },
            fixture.config.clone(),
        )
        .await;
        assert!(matches!(
            result,
            Err(vibemuxd::chat_session::RuntimeError::InvalidConfiguration)
        ));
        assert!(
            !fixture.vendor.join("chat_fixture.json").exists(),
            "no configuration/effect before guarded admission"
        );
        assert!(
            !PathBuf::from(&fixture.workspace.path)
                .join(FIXTURE_APPROVAL_FILE)
                .exists()
        );
        fixture
            .chats
            .change_execution_process(
                vibemux_store::ChatMutation {
                    operation_id: Uuid::new_v4(),
                    chat_id: fixture.owner.chat_id,
                    expected_generation: 1,
                    timestamp: OffsetDateTime::now_utc(),
                },
                fixture.binding.owner_fence,
                vibemux_store::ChatExecutionProcessChange::Released {
                    cleanup_verified: true,
                    closed: true,
                },
            )
            .await
            .expect("explicit no-launch release");
        fixture.cleanup_closed().await;
    }
}
