#![cfg(feature = "test_helpers")]

use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use vibemux_plugin_protocol::{
    PROTOCOL_MAJOR, PROTOCOL_MINOR,
    frame::FrameCodecConfig,
    manifest::{ManifestPluginKind, PluginManifest, SupportedPlatform},
    negotiation::CorePluginPolicy,
    wire::{Envelope, PluginKind, Ready, Request, envelope},
};
use vibemux_plugin_supervisor::{
    PluginSupervisorConfig, PluginSupervisorError, ResolvedPluginLaunch, spawn_plugin,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_mock_handshake_echo_environment_cancel_and_shutdown() {
    let mut session = spawn_plugin(config("normal", 4096))
        .await
        .expect("spawn normal plugin");
    let heartbeat = session.receive().await.expect("initial heartbeat");
    assert!(matches!(heartbeat.body, Some(envelope::Body::Heartbeat(_))));
    assert_eq!(session.heartbeat_sequence(), Some(1));
    assert!(session.heartbeat_age().is_some());
    assert!(session.heartbeat_is_fresh(Duration::from_secs(1)));
    let duplicate_ready = Envelope {
        protocol_major: PROTOCOL_MAJOR,
        protocol_minor: PROTOCOL_MINOR,
        message_id: "ready_again".to_string(),
        correlation_id: Some("session_normal".to_string()),
        causation_id: None,
        body: Some(envelope::Body::Ready(Ready {
            session_id: "session_normal".to_string(),
        })),
    };
    assert_eq!(
        session
            .try_send(duplicate_ready)
            .expect_err("handshake body after activation"),
        PluginSupervisorError::Protocol {
            code: "plugin_invalid_transition".to_string(),
        }
    );

    session
        .try_send(request("request_1", "mock:echo", b"hello"))
        .expect("send echo");
    let response = session.receive().await.expect("echo response");
    let Some(envelope::Body::Response(response)) = response.body else {
        panic!("response body");
    };
    assert_eq!(response.request_id, "request_1");
    assert_eq!(response.payload, b"hello");

    session
        .try_send(request("request_2", "mock:environment", b""))
        .expect("send environment request");
    let response = session.receive().await.expect("environment response");
    let Some(envelope::Body::Response(response)) = response.body else {
        panic!("environment response body");
    };
    assert_eq!(response.payload, b"allowed=visible;path_present=false");

    session
        .try_cancel("request_3", "core:cancelled")
        .expect("send cancel");
    let event = session.receive().await.expect("cancel event");
    assert!(matches!(event.body, Some(envelope::Body::Event(_))));

    let report = session.shutdown().await.expect("graceful shutdown");
    assert!(report.graceful);
    assert!(report.success);
    assert_eq!(report.exit_code, Some(0));
    assert!(!report.stderr.truncated);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stderr_flood_is_bounded_without_corrupting_stdout() {
    let mut session = spawn_plugin(config("stderr_flood", 1024))
        .await
        .expect("spawn stderr flood plugin");
    session.receive().await.expect("heartbeat remains valid");
    session
        .try_send(request("request_1", "mock:echo", b"stdout_ok"))
        .expect("send echo after stderr flood");
    let response = session
        .receive()
        .await
        .expect("response after stderr flood");
    assert!(matches!(response.body, Some(envelope::Body::Response(_))));
    let report = session.shutdown().await.expect("shutdown flood plugin");
    assert_eq!(report.stderr.captured_bytes, 1024);
    assert!(report.stderr.truncated);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_session_outbound_saturation_reports_queue_full() {
    let mut supervisor_config = config("quiet", 1024);
    supervisor_config.outbound_capacity = 1;
    let mut session = spawn_plugin(supervisor_config)
        .await
        .expect("spawn quiet plugin");
    session.receive().await.expect("initial heartbeat");
    assert_eq!(
        session
            .receive_with_timeout(Duration::from_millis(50))
            .await
            .expect_err("quiet plugin receive deadline"),
        PluginSupervisorError::ReceiveTimeout
    );
    let mut observed_full = false;
    for sequence in 0..16 {
        let envelope = request(
            &format!("request_{sequence}"),
            "mock:echo",
            &vec![b'x'; 256 * 1024],
        );
        match session.try_send(envelope) {
            Ok(()) => tokio::task::yield_now().await,
            Err(PluginSupervisorError::QueueFull) => {
                observed_full = true;
                break;
            }
            Err(error) => panic!("unexpected queue error: {error}"),
        }
    }
    assert!(observed_full, "bounded outbound queue must saturate");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_oversized_wrong_ready_and_handshake_timeout_are_isolated() {
    for (mode, expected_code) in [
        ("malformed", "plugin_frame_too_large"),
        ("oversized", "plugin_frame_too_large"),
        ("wrong_ready", "plugin_invalid_transition"),
        ("hang_handshake", "plugin_supervisor_handshake_timeout"),
    ] {
        let error = match spawn_plugin(config(mode, 1024)).await {
            Err(error) => error,
            Ok(session) => {
                drop(session);
                panic!("fixture must fail handshake: {mode}");
            }
        };
        assert_eq!(error.code(), expected_code, "mode {mode}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crash_report_and_shutdown_timeout_do_not_crash_supervisor() {
    let mut crashed = spawn_plugin(config("crash", 1024))
        .await
        .expect("spawn crashing plugin");
    let report = crashed
        .wait_for_exit(Duration::from_secs(2))
        .await
        .expect("crash report");
    assert!(!report.graceful);
    assert!(!report.success);
    assert_eq!(report.exit_code, Some(7));
    assert!(report.stderr.captured_bytes > 0);

    let mut hanging = config("hang_shutdown", 1024);
    hanging.shutdown_timeout = Duration::from_millis(150);
    let mut session = spawn_plugin(hanging)
        .await
        .expect("spawn shutdown hang plugin");
    session.receive().await.expect("heartbeat");
    assert_eq!(
        session
            .shutdown()
            .await
            .expect_err("shutdown must time out"),
        PluginSupervisorError::ShutdownTimeout
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_heartbeat_sequence_is_rejected() {
    let mut session = spawn_plugin(config("duplicate_heartbeat", 1024))
        .await
        .expect("spawn duplicate heartbeat plugin");
    session.receive().await.expect("first heartbeat");
    let error = session
        .receive()
        .await
        .expect_err("duplicate heartbeat must fail");
    assert_eq!(error.code(), "plugin_heartbeat_not_monotonic");
}

fn config(mode: &str, stderr_limit: usize) -> PluginSupervisorConfig {
    let manifest = PluginManifest {
        schema_version: 1,
        plugin_id: "mock.harness".to_string(),
        plugin_version: "1.0.0".to_string(),
        kind: ManifestPluginKind::Harness,
        entry_point: vec!["vibemux_mock_plugin".to_string(), mode.to_string()],
        capabilities: vec!["mock:cancel".to_string(), "mock:echo".to_string()],
        requested_permissions: vec![],
        supported_platforms: vec![current_platform()],
        protocol_major: PROTOCOL_MAJOR,
        minimum_protocol_minor: 0,
        maximum_protocol_minor: 0,
    };
    let mut environment = BTreeMap::new();
    environment.insert("ALLOWED_VALUE".to_string(), "visible".to_string());
    let launch = ResolvedPluginLaunch::new(
        &manifest,
        PathBuf::from(env!("CARGO_BIN_EXE_vibemux_mock_plugin")),
        std::env::current_dir().expect("current directory"),
        environment,
    )
    .expect("resolved launch");
    let policy = CorePluginPolicy::new(
        vec!["mock:cancel".to_string(), "mock:echo".to_string()],
        vec![],
        FrameCodecConfig::default(),
        8,
        1000,
        current_platform(),
    )
    .expect("core policy");
    let mut config =
        PluginSupervisorConfig::new(manifest, launch, policy, format!("session_{mode}"));
    config.handshake_timeout = Duration::from_millis(500);
    config.receive_timeout = Duration::from_secs(2);
    config.shutdown_timeout = Duration::from_secs(2);
    config.outbound_capacity = 4;
    config.inbound_capacity = 4;
    config.stderr_limit_bytes = stderr_limit;
    config
}

fn request(request_id: &str, method: &str, payload: &[u8]) -> Envelope {
    Envelope {
        protocol_major: PROTOCOL_MAJOR,
        protocol_minor: PROTOCOL_MINOR,
        message_id: format!("message_{request_id}"),
        correlation_id: Some("session_normal".to_string()),
        causation_id: None,
        body: Some(envelope::Body::Request(Request {
            request_id: request_id.to_string(),
            method: method.to_string(),
            payload: payload.to_vec(),
            deadline_unix_ms: 1,
            idempotency_key: Some(format!("idempotency_{request_id}")),
        })),
    }
}

const fn current_platform() -> SupportedPlatform {
    #[cfg(windows)]
    {
        SupportedPlatform::Windows
    }
    #[cfg(target_os = "linux")]
    {
        SupportedPlatform::Linux
    }
    #[cfg(target_os = "macos")]
    {
        SupportedPlatform::Macos
    }
}

#[test]
fn mock_kind_matches_protocol_kind() {
    assert_eq!(
        i32::from(PluginKind::from(ManifestPluginKind::Harness)),
        i32::from(PluginKind::Harness)
    );
}

#[tokio::test]
async fn zero_queue_bound_is_rejected_before_spawn() {
    let mut invalid = config("normal", 1024);
    invalid.outbound_capacity = 0;
    let error = match spawn_plugin(invalid).await {
        Err(error) => error,
        Ok(session) => {
            drop(session);
            panic!("zero queue bound must fail");
        }
    };
    assert_eq!(error, PluginSupervisorError::InvalidConfiguration);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_accepts_queued_heartbeat_and_cancel_event() {
    let mut session = spawn_plugin(config("normal", 1024))
        .await
        .expect("spawn plugin with queued initial heartbeat");
    session
        .try_cancel("pending_cancel", "core:cancelled")
        .expect("queue cancellation before drain");
    let report = session.shutdown().await.expect("drain queued traffic");
    assert!(report.graceful);
    assert!(report.success);
    assert_eq!(report.exit_code, Some(0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_termination_reaps_unresponsive_child() {
    let mut session = spawn_plugin(config("quiet", 1024))
        .await
        .expect("spawn unresponsive plugin");
    session.receive().await.expect("initial heartbeat");
    let report = tokio::time::timeout(Duration::from_secs(3), session.terminate())
        .await
        .expect("termination remains bounded")
        .expect("exact child exit report after forced termination");
    assert!(!report.graceful);
    assert!(!report.success);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_rejects_wrong_session_acknowledgement() {
    let mut session = spawn_plugin(config("wrong_shutdown_ack", 1024))
        .await
        .expect("spawn wrong-acknowledgement plugin");
    session.receive().await.expect("initial heartbeat");
    let error = tokio::time::timeout(Duration::from_secs(3), session.shutdown())
        .await
        .expect("rejected acknowledgement cleanup remains bounded")
        .expect_err("shutdown acknowledgement must match the session");
    assert_eq!(error.code(), "plugin_invalid_transition");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_termination_after_exit_report_returns_closed_without_panicking() {
    let mut session = spawn_plugin(config("crash", 1024))
        .await
        .expect("spawn crashing plugin");
    let report = session
        .wait_for_exit(Duration::from_secs(2))
        .await
        .expect("reap child crash");
    assert_eq!(report.exit_code, Some(7));
    assert_eq!(
        session.terminate().await.expect_err("already reaped child"),
        PluginSupervisorError::SessionClosed
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_heartbeat_crash_and_unsolicited_event_fixtures_use_real_wire_frames() {
    let mut crashing = spawn_plugin(config("crash_after_heartbeat", 1024))
        .await
        .expect("spawn delayed crash plugin");
    crashing.receive().await.expect("heartbeat before crash");
    let report = crashing
        .wait_for_exit(Duration::from_secs(2))
        .await
        .expect("delayed crash report");
    assert_eq!(report.exit_code, Some(7));
    assert!(!report.graceful);

    let mut unsolicited = spawn_plugin(config("unsolicited_event", 1024))
        .await
        .expect("spawn unsolicited event plugin");
    unsolicited.receive().await.expect("initial heartbeat");
    let event = unsolicited.receive().await.expect("unsolicited event");
    let Some(envelope::Body::Event(event)) = event.body else {
        panic!("event body");
    };
    assert_eq!(event.event_type, "task:completed");
    assert!(
        event
            .payload
            .windows(b"private_mock_prompt".len())
            .any(|window| window == b"private_mock_prompt")
    );
    assert!(
        unsolicited
            .shutdown()
            .await
            .expect("graceful shutdown")
            .graceful
    );
}
