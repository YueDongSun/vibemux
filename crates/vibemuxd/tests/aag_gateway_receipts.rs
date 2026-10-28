//! Request-bound AAG v2 transport checks against an owned synthetic loopback peer.
#![cfg(feature = "test_helpers")]

mod aag_bound_fixture_support;
#[allow(dead_code)]
#[path = "../src/workflow/aag_gateway.rs"]
mod aag_gateway;

use aag_bound_fixture_support::{BoundFixtureServer, FIXTURE_TOKEN, FixtureMode};
use aag_gateway::{
    Aag_Call_Class, Aag_Call_Request, Aag_Call_Result, Aag_Credential, Aag_Fixture_Permit,
    Aag_Gateway_Error, Aag_Model_Gateway,
};
use tokio::sync::watch;
use uuid::Uuid;
use vibemux_workflow::{
    Sha256Digest,
    gateway::{
        CallCorrelation, CallOutcome, GATEWAY_CONFIG_SCHEMA_VERSION, GatewayConfig,
        GatewayProtocol, LiveAdmission, ProviderAttribution, RetryDecision, RouteBinding,
        RouteRole,
    },
};

fn digest() -> Sha256Digest {
    Sha256Digest::of(b"aag_v2_fixture_configuration")
}
fn config(server: &BoundFixtureServer, policy: LiveAdmission) -> GatewayConfig {
    GatewayConfig {
        schema_version: GATEWAY_CONFIG_SCHEMA_VERSION,
        endpoint: server.origin().to_string(),
        credential_env: "AAG_V2_FIXTURE_TOKEN".into(),
        bindings: vec![
            RouteBinding {
                role: RouteRole::WorkerA,
                model_alias: "alias_a".into(),
                protocol: GatewayProtocol::OpenaiChatCompletions,
                max_output_tokens: 64,
                timeout_ms: 1_000,
            },
            RouteBinding {
                role: RouteRole::Compiler,
                model_alias: "alias_anthropic".into(),
                protocol: GatewayProtocol::AnthropicMessages,
                max_output_tokens: 64,
                timeout_ms: 1_000,
            },
        ],
        strict_routing: true,
        live_admission: policy,
    }
}
fn gateway(server: &BoundFixtureServer, policy: LiveAdmission) -> Aag_Model_Gateway {
    Aag_Model_Gateway::new(
        config(server, policy),
        digest(),
        Aag_Credential::explicit(FIXTURE_TOKEN.into()).expect("synthetic credential"),
    )
    .expect("bounded gateway")
}
fn request(role: RouteRole) -> Aag_Call_Request {
    Aag_Call_Request {
        class: Aag_Call_Class::Live,
        role,
        correlation: CallCorrelation {
            workflow_id: Uuid::from_u128(1),
            call_id: Uuid::new_v4(),
            attempt: 1,
        },
        expected_config_digest: digest(),
        expected_route_generation: None,
        system: "fixture system text".into(),
        user: "fixture user text".into(),
        stream: false,
    }
}
fn cancellation() -> watch::Receiver<bool> {
    watch::channel(false).1
}
async fn server() -> BoundFixtureServer {
    BoundFixtureServer::start(&["alias_a", "alias_anthropic"], "fixture answer").await
}
fn assert_uncertain(result: &Aag_Call_Result, expected: Aag_Gateway_Error) {
    assert_eq!(result.outcome, CallOutcome::FailedAfterOutput, "{result:?}");
    assert_eq!(result.error, Some(expected));
    assert!(result.completion.is_none());
    assert!(matches!(result.retry, RetryDecision::NoAutomaticRetry(_)));
    let receipt = result.receipt.as_ref().expect("failure receipt");
    assert!(receipt.response_sha256.is_some());
    assert_eq!(receipt.http_status, Some(200));
    assert_eq!(receipt.effective_provider, ProviderAttribution::Unknown);
}

#[tokio::test]
async fn live_policy_is_explicit_and_default_refuses_before_http() {
    let server = server().await;
    let result = gateway(&server, LiveAdmission::Disabled)
        .call(request(RouteRole::WorkerA), cancellation())
        .await;
    assert_eq!(
        result.error,
        Some(Aag_Gateway_Error::LiveAttestationUnavailable)
    );
    assert_eq!(server.counts().requests, 0);
    server.close().await;
}

#[tokio::test]
async fn bound_json_chat_and_messages_release_only_gateway_observed_evidence() {
    let server = server().await;
    let gateway = gateway(&server, LiveAdmission::BoundReceiptV2);
    for role in [RouteRole::WorkerA, RouteRole::Compiler] {
        let result = gateway.call(request(role), cancellation()).await;
        assert_eq!(result.outcome, CallOutcome::Completed, "{result:?}");
        let completion = result.completion.expect("matched completion");
        assert_eq!(completion.text, "fixture answer");
        let receipt = result.receipt.expect("model receipt");
        let ProviderAttribution::GatewayObserved(evidence) = &receipt.effective_provider else {
            panic!("missing bound gateway evidence");
        };
        assert_eq!(evidence.admission().generation, 7);
        assert_eq!(evidence.wire_model(), "wire_fixture_model");
        assert_eq!(evidence.provider_reported_model(), Some("upstream_claim"));
        assert_ne!(
            completion.reported_model.as_deref(),
            evidence.provider_reported_model()
        );
        let serialized = serde_json::to_string(&receipt).expect("content-free receipt");
        assert!(!serialized.contains(FIXTURE_TOKEN));
        assert!(!serialized.contains("fixture user text"));
        assert!(!serialized.contains("fixture answer"));
        assert!(!serialized.contains("x-aag-receipt-capability"));
    }
    assert_eq!(server.counts().upstream_calls, 2);
    assert_eq!(server.counts().receipt_reads, 2);
    assert!(
        server
            .summaries()
            .iter()
            .all(|entry| entry.gateway_auth_valid)
    );
    assert!(
        server
            .summaries()
            .iter()
            .filter(|entry| entry.method == "POST")
            .all(|entry| entry.receipt_opt_in
                && entry.route_precondition
                && !entry.lookup_capability)
    );
    server.close().await;
}

#[tokio::test]
async fn bound_sse_is_consumed_to_terminal_before_receipt_release() {
    let server = server().await;
    let gateway = gateway(&server, LiveAdmission::BoundReceiptV2);
    let mut input = request(RouteRole::WorkerA);
    input.stream = true;
    let result = gateway.call(input, cancellation()).await;
    assert_eq!(result.outcome, CallOutcome::Completed, "{result:?}");
    assert_eq!(
        result.completion.expect("SSE completion").text,
        "fixture answer"
    );
    assert!(matches!(
        result.receipt.expect("receipt").effective_provider,
        ProviderAttribution::GatewayObserved(_)
    ));
    assert_eq!(server.counts().upstream_calls, 1);
    assert_eq!(server.counts().receipt_reads, 1);
    server.close().await;
}

#[tokio::test]
async fn fixture_label_still_requires_owned_listener_with_bound_policy() {
    let server = server().await;
    let gateway = gateway(&server, LiveAdmission::BoundReceiptV2);
    let mut input = request(RouteRole::WorkerA);
    input.class = Aag_Call_Class::Fixture;
    let denied = gateway.call(input.clone(), cancellation()).await;
    assert_eq!(denied.error, Some(Aag_Gateway_Error::FixturePermitRequired));
    assert_eq!(server.counts().requests, 0);
    let gateway = gateway
        .with_fixture_permit(
            Aag_Fixture_Permit::from_owned_listener(server.listener()).expect("owned listener"),
        )
        .expect("same origin");
    let result = gateway.call(input, cancellation()).await;
    assert_eq!(result.outcome, CallOutcome::Completed, "{result:?}");
    server.close().await;
}

#[tokio::test]
async fn unsupported_or_malformed_capabilities_never_send_inference() {
    for mode in [
        FixtureMode::CapabilitiesV1,
        FixtureMode::CapabilitiesMalformed,
    ] {
        let server = server().await;
        server.set_mode(mode);
        let result = gateway(&server, LiveAdmission::BoundReceiptV2)
            .call(request(RouteRole::WorkerA), cancellation())
            .await;
        assert_eq!(result.outcome, CallOutcome::Blocked);
        assert_eq!(result.error, Some(Aag_Gateway_Error::ReceiptInvalid));
        assert_eq!(server.counts().inference_requests, 0);
        assert_eq!(server.counts().upstream_calls, 0);
        server.close().await;
    }
}

#[tokio::test]
async fn either_health_or_capability_pin_blocks_strict_routing() {
    for caps_only in [true, false] {
        let server = server().await;
        if caps_only {
            server.set_mode(FixtureMode::CapabilitiesPinned);
        } else {
            server.set_pin(true);
        }
        let result = gateway(&server, LiveAdmission::BoundReceiptV2)
            .call(request(RouteRole::WorkerA), cancellation())
            .await;
        assert_eq!(result.error, Some(Aag_Gateway_Error::RouteUnavailable));
        assert_eq!(server.counts().inference_requests, 0);
        server.close().await;
    }
}

#[tokio::test]
async fn native_generation_changes_are_part_of_the_expected_route_fingerprint() {
    let server = server().await;
    let gateway = gateway(&server, LiveAdmission::BoundReceiptV2);
    let first = gateway
        .call(request(RouteRole::WorkerA), cancellation())
        .await;
    assert_eq!(first.outcome, CallOutcome::Completed);
    let generation = first.observation.expect("observation").route_generation;
    server.set_generation(8);
    let mut next = request(RouteRole::WorkerA);
    next.expected_route_generation = Some(generation);
    let result = gateway.call(next, cancellation()).await;
    assert_eq!(result.error, Some(Aag_Gateway_Error::RouteChanged));
    assert_eq!(server.counts().inference_requests, 1);
    server.close().await;
}

#[tokio::test]
async fn admission_change_is_refused_without_fallback_or_upstream_effect() {
    let server = server().await;
    server.set_mode(FixtureMode::PreconditionChanged);
    let result = gateway(&server, LiveAdmission::BoundReceiptV2)
        .call(request(RouteRole::WorkerA), cancellation())
        .await;
    assert_eq!(result.error, Some(Aag_Gateway_Error::HttpStatus));
    assert!(result.completion.is_none());
    assert!(matches!(result.retry, RetryDecision::NoAutomaticRetry(_)));
    assert_eq!(server.counts().inference_requests, 1);
    assert_eq!(server.counts().upstream_calls, 0);
    assert_eq!(server.counts().receipt_reads, 0);
    server.close().await;
}

#[tokio::test]
async fn missing_wrong_or_partial_receipt_never_releases_or_repeats_inference() {
    let modes = [
        FixtureMode::ReceiptMissing,
        FixtureMode::ReceiptWrongNonce,
        FixtureMode::ReceiptWrongInstance,
        FixtureMode::ReceiptWrongGeneration,
        FixtureMode::ReceiptWrongSelector,
        FixtureMode::ReceiptWrongSurface,
        FixtureMode::ReceiptWrongRequestId,
        FixtureMode::ReceiptDropped,
        FixtureMode::ReceiptTruncated,
        FixtureMode::ReceiptIncomplete,
    ];
    for mode in modes {
        let server = server().await;
        server.set_mode(mode);
        let result = gateway(&server, LiveAdmission::BoundReceiptV2)
            .call(request(RouteRole::WorkerA), cancellation())
            .await;
        assert_uncertain(
            &result,
            if mode == FixtureMode::ReceiptMissing {
                Aag_Gateway_Error::ReceiptUnavailable
            } else {
                Aag_Gateway_Error::ReceiptInvalid
            },
        );
        assert_eq!(server.counts().upstream_calls, 1);
        assert_eq!(server.counts().receipt_reads, 1);
        server.close().await;
    }
}

#[tokio::test]
async fn response_identity_headers_are_unique_required_and_bound_to_lookup() {
    for mode in [
        FixtureMode::ResponseMissingVersion,
        FixtureMode::ResponseDuplicateNonce,
        FixtureMode::ResponseWrongRequestId,
    ] {
        let server = server().await;
        server.set_mode(mode);
        let result = gateway(&server, LiveAdmission::BoundReceiptV2)
            .call(request(RouteRole::WorkerA), cancellation())
            .await;
        assert_uncertain(&result, Aag_Gateway_Error::ReceiptInvalid);
        assert_eq!(server.counts().upstream_calls, 1);
        assert_eq!(
            server.counts().receipt_reads,
            usize::from(mode == FixtureMode::ResponseWrongRequestId)
        );
        server.close().await;
    }
}

#[tokio::test]
async fn expired_or_predating_receipt_is_uncertain_even_with_matching_identity() {
    for mode in [FixtureMode::ReceiptExpired, FixtureMode::ReceiptOld] {
        let server = server().await;
        server.set_mode(mode);
        let result = gateway(&server, LiveAdmission::BoundReceiptV2)
            .call(request(RouteRole::WorkerA), cancellation())
            .await;
        assert_uncertain(&result, Aag_Gateway_Error::ReceiptExpired);
        assert_eq!(server.counts().upstream_calls, 1);
        server.close().await;
    }
}

#[tokio::test]
async fn cancellation_during_receipt_read_retains_response_evidence_without_completion() {
    let server = server().await;
    server.set_mode(FixtureMode::ReceiptDelay);
    let gateway = gateway(&server, LiveAdmission::BoundReceiptV2);
    let (cancel, receiver) = watch::channel(false);
    let call =
        tokio::spawn(async move { gateway.call(request(RouteRole::WorkerA), receiver).await });
    server.wait_for_receipt_read().await;
    cancel.send(true).expect("cancel call");
    let result = call.await.expect("call joined");
    assert_uncertain(&result, Aag_Gateway_Error::Cancelled);
    assert_eq!(server.counts().upstream_calls, 1);
    assert_eq!(server.counts().receipt_reads, 1);
    server.close().await;
}

#[tokio::test]
async fn original_call_deadline_also_bounds_receipt_retrieval() {
    let server = server().await;
    server.set_mode(FixtureMode::ReceiptDelay);
    let result = gateway(&server, LiveAdmission::BoundReceiptV2)
        .call(request(RouteRole::WorkerA), cancellation())
        .await;
    assert_uncertain(&result, Aag_Gateway_Error::Deadline);
    assert_eq!(server.counts().upstream_calls, 1);
    server.close().await;
}

/// Invoked only by the task-owned actual AAG Axum fixture, never a default test.
#[tokio::test]
#[ignore = "requires the owned AAG v2 source fixture and its synthetic credential"]
async fn consume_actual_aag_v2_owner() {
    let origin = std::env::var("VIBEMUX_AAG_V2_FIXTURE_ORIGIN").expect("owned fixture origin");
    let token =
        std::env::var("VIBEMUX_AAG_V2_FIXTURE_TOKEN").expect("synthetic fixture credential");
    let mut config = GatewayConfig {
        schema_version: GATEWAY_CONFIG_SCHEMA_VERSION,
        endpoint: origin,
        credential_env: "AAG_V2_FIXTURE_REFERENCE".into(),
        bindings: vec![RouteBinding {
            role: RouteRole::Compiler,
            model_alias: "admission_alias".into(),
            protocol: GatewayProtocol::OpenaiChatCompletions,
            max_output_tokens: 64,
            timeout_ms: 5_000,
        }],
        strict_routing: true,
        live_admission: LiveAdmission::Disabled,
    };
    let disabled = Aag_Model_Gateway::new(
        config.clone(),
        digest(),
        Aag_Credential::explicit(token.clone()).expect("synthetic credential"),
    )
    .expect("disabled gateway");
    let refused = disabled
        .call(request(RouteRole::Compiler), cancellation())
        .await;
    assert_eq!(
        refused.error,
        Some(Aag_Gateway_Error::LiveAttestationUnavailable)
    );
    config.live_admission = LiveAdmission::BoundReceiptV2;
    let enabled = Aag_Model_Gateway::new(
        config,
        digest(),
        Aag_Credential::explicit(token).expect("synthetic credential"),
    )
    .expect("receipt-required gateway");
    let result = enabled
        .call(request(RouteRole::Compiler), cancellation())
        .await;
    assert_eq!(result.outcome, CallOutcome::Completed, "{result:?}");
    assert_eq!(
        result.completion.expect("matched source completion").text,
        "aag_v2_fixture_ok"
    );
    let snapshot = result
        .observation
        .expect("pre-request observation")
        .admission_snapshot
        .expect("v2 snapshot");
    let receipt = result.receipt.expect("content-free receipt");
    assert_eq!(receipt.requested_alias, "admission_alias");
    let ProviderAttribution::GatewayObserved(evidence) = receipt.effective_provider else {
        panic!("gateway route evidence required");
    };
    assert_eq!(evidence.admission(), &snapshot);
    assert_eq!(snapshot.generation, 1);
    assert!(!snapshot.pinned_provider_present);
    assert_eq!(evidence.wire_model(), "wire_model");
    assert_eq!(evidence.provider_reported_model(), Some("reported_model"));
    assert_eq!(
        evidence.selected_identity().provider_ref.as_deref(),
        Some("provider_a")
    );
    assert_eq!(
        evidence
            .receipt()
            .routing_receipt
            .requested_selector
            .as_deref(),
        Some("admission_alias")
    );
    assert_eq!(
        evidence.upstream_model_attestation(),
        vibemux_workflow::gateway_receipt::AttestationState::Unavailable
    );
    println!("VIBEMUX_AAG_V2_OWNER_FIXTURE_PASS");
}
