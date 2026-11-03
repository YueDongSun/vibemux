use super::*;
use vibemuxd::{
    chat_session::{ChatService, RuntimeHandle},
    control::{ChatApprovalRequest, ChatApprovalResponse, ChatExecutionRequest, ControlClient},
};

impl Fixture {
    fn control(&self) -> (ControlClient, ChatService) {
        let FixtureWriter::Daemon(server) = &self.writer else {
            panic!("Control-owned fixture")
        };
        (
            ControlClient::from_descriptor(server.descriptor_path()).expect("authenticated client"),
            server.fixture_chat_context().expect("same owner").1,
        )
    }
    async fn service_runtime<F>(&self, service: &ChatService, wrap: F) -> RuntimeHandle
    where
        F: FnOnce(
                std::sync::Arc<dyn vibemuxd::chat_session::ChatRuntimeSink>,
            ) -> std::sync::Arc<dyn vibemuxd::chat_session::ChatRuntimeSink>
            + Send
            + 'static,
    {
        service
            .start_fixture_approval_runtime(
                self.binding.clone(),
                FixtureApprovalSettings {
                    project_root: self.project.clone(),
                    git: self.git.clone(),
                    vendor_directory: self.vendor.clone(),
                    trampoline: PathBuf::from(env!("CARGO_BIN_EXE_vibemux_launch_trampoline")),
                    runtime_limits: RuntimeLimits::default(),
                },
                self.config.clone(),
                WAIT,
                wrap,
            )
            .await
            .expect("guarded service-owned fixture")
    }
}

async fn pending(
    client: &ControlClient,
    fixture: &Fixture,
    request: Uuid,
) -> vibemuxd::chat_session::approval_api::ChatApprovalStatus {
    timeout(WAIT, async {
        loop {
            let value = client
                .chat_approval(&ChatApprovalRequest::Pending {
                    chat_id: fixture.owner.chat_id,
                    generation: 1,
                    request_id: request,
                })
                .await
                .expect("pending query");
            if let ChatApprovalResponse::Pending {
                approval: Some(value),
                ..
            } = value
            {
                return value;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("offered challenge through IPC")
}

fn decision(
    fixture: &Fixture,
    approval: &vibemuxd::chat_session::approval_api::ChatApprovalStatus,
    operation_id: Uuid,
    choice: ChatApprovalDecision,
) -> ChatApprovalRequest {
    ChatApprovalRequest::Decide {
        chat_id: fixture.owner.chat_id,
        approval_id: approval.approval_id,
        generation: 1,
        request_id: approval.request_id,
        expected_version: approval.version,
        operation_id,
        decision: choice,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authenticated_control_decisions_reach_one_use_fixture_delivery_and_recover_without_a_live_actor()
 {
    for choice in [ChatApprovalDecision::AllowOnce, ChatApprovalDecision::Deny] {
        let fixture = Fixture::with_control().await;
        let (client, service) = fixture.control();
        let handle = fixture.service_runtime(&service, |sink| sink).await;
        let request = fixture.send_handle(&handle, WAIT).await;
        let approval = pending(&client, &fixture, request).await;
        assert!(approval.decidable);
        let metadata = serde_json::to_string(&approval).expect("safe metadata");
        for hidden in [
            "profile_sha256",
            "evidence_ref",
            "vendor_request_id",
            "tool_use_id",
            "file_path",
        ] {
            assert!(!metadata.contains(hidden));
        }
        let content = client
            .chat_approval(&ChatApprovalRequest::Content {
                chat_id: fixture.owner.chat_id,
                approval_id: approval.approval_id,
                generation: 1,
                request_id: request,
                byte_offset: 0,
            })
            .await
            .expect("explicit input read");
        let ChatApprovalResponse::Content { page, .. } = content else {
            panic!("content shape")
        };
        assert!(page.complete && page.content_error.is_none());
        let text = page.text.as_deref().expect("private JSON input");
        let input: serde_json::Value = serde_json::from_str(text).expect("JSON input");
        assert_eq!(input["content"], FIXTURE_APPROVAL_CONTENT);
        assert!(!format!("{page:?}").contains(FIXTURE_APPROVAL_CONTENT));
        let middle = text.find('中').expect("Unicode input path") as u32 + 1;
        assert!(
            client
                .chat_approval(&ChatApprovalRequest::Content {
                    chat_id: fixture.owner.chat_id,
                    approval_id: approval.approval_id,
                    generation: 1,
                    request_id: request,
                    byte_offset: middle
                })
                .await
                .is_err()
        );
        let operation = Uuid::new_v4();
        let mut wrong = decision(&fixture, &approval, Uuid::new_v4(), choice);
        if let ChatApprovalRequest::Decide {
            expected_version, ..
        } = &mut wrong
        {
            *expected_version += 1;
        }
        assert!(client.chat_approval(&wrong).await.is_err());
        assert!(
            !fixture
                .workspace_path()
                .join(FIXTURE_APPROVAL_FILE)
                .exists()
        );
        let sent = decision(&fixture, &approval, operation, choice);
        let response = client
            .chat_approval(&sent)
            .await
            .expect("explicit human choice");
        let ChatApprovalResponse::Decision { receipt, .. } = response else {
            panic!("decision receipt")
        };
        assert_eq!(receipt.approval.decision, Some(choice));
        assert!(!receipt.duplicate);
        timeout(WAIT, async {
            loop {
                let value = fixture
                    .chats
                    .execution_attempt(request)
                    .await
                    .expect("attempt")
                    .expect("admitted");
                if value.phase.is_terminal() {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("terminal attempt");
        service
            .close_execution_process(fixture.owner.chat_id, 1)
            .await
            .expect("closed exact service owner");
        let replay = client
            .chat_approval(&sent)
            .await
            .expect("historical replay without actor");
        let ChatApprovalResponse::Decision {
            receipt: replayed, ..
        } = replay
        else {
            panic!("receipt")
        };
        assert!(replayed.duplicate);
        assert_eq!(replayed.sequence, receipt.sequence);
        assert_eq!(
            replayed.approval.delivery,
            Some(ChatApprovalDelivery::Written)
        );
        let query = ChatApprovalRequest::Receipt {
            chat_id: fixture.owner.chat_id,
            approval_id: approval.approval_id,
            generation: 1,
            request_id: request,
            expected_version: approval.version,
            operation_id: operation,
            decision: choice,
        };
        assert!(matches!(
            client
                .chat_approval(&query)
                .await
                .expect("readonly receipt"),
            ChatApprovalResponse::Receipt {
                receipt: Some(_),
                ..
            }
        ));
        let mut changed = sent.clone();
        if let ChatApprovalRequest::Decide { decision, .. } = &mut changed {
            *decision = if choice == ChatApprovalDecision::Deny {
                ChatApprovalDecision::AllowOnce
            } else {
                ChatApprovalDecision::Deny
            };
        }
        assert!(client.chat_approval(&changed).await.is_err());
        assert_eq!(
            fixture
                .workspace_path()
                .join(FIXTURE_APPROVAL_FILE)
                .exists(),
            choice == ChatApprovalDecision::AllowOnce
        );
        assert_eq!(
            fixture
                .chats
                .execution_owner(fixture.owner.chat_id)
                .await
                .expect("owner")
                .expect("retained")
                .run
                .status(),
            RunStatus::Running
        );
        drop(handle);
        drop(service);
        drop(client);
        fixture.cleanup_closed().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn control_cancellation_remains_responsive_while_the_decision_callback_is_parked() {
    use super::boundary_cases::{Gate, GateStage, GatedSink};
    let fixture = Fixture::with_control().await;
    let (client, service) = fixture.control();
    let gate = Gate::new(GateStage::Decision);
    let for_sink = gate.clone();
    let handle = fixture
        .service_runtime(&service, move |inner| {
            std::sync::Arc::new(GatedSink {
                inner,
                gate: for_sink,
            })
        })
        .await;
    let request = fixture.send_handle(&handle, WAIT).await;
    let approval = pending(&client, &fixture, request).await;
    let sent = decision(
        &fixture,
        &approval,
        Uuid::new_v4(),
        ChatApprovalDecision::AllowOnce,
    );
    let other = client.clone();
    let decision_job = tokio::spawn(async move { other.chat_approval(&sent).await });
    gate.observed().await;
    timeout(
        Duration::from_secs(2),
        client.chat_execution(&ChatExecutionRequest::Cancel {
            chat_id: fixture.owner.chat_id,
            generation: 1,
            request_id: request,
        }),
    )
    .await
    .expect("cancel is not queued behind the decision wait")
    .expect("canonical cancel");
    assert!(
        !fixture
            .workspace_path()
            .join(FIXTURE_APPROVAL_FILE)
            .exists()
    );
    gate.open();
    decision_job
        .await
        .expect("decision observer joined")
        .expect("committed decision retained");
    timeout(WAIT, async {
        loop {
            let value = fixture
                .chats
                .approval_metadata(fixture.owner.chat_id, approval.approval_id)
                .await
                .expect("metadata");
            if value.delivery == Some(ChatApprovalDelivery::Unknown) {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("unknown delivery settled");
    service
        .close_execution_process(fixture.owner.chat_id, 1)
        .await
        .expect("close after reconciliation");
    drop(handle);
    drop(service);
    drop(client);
    fixture.cleanup_closed().await;
}
