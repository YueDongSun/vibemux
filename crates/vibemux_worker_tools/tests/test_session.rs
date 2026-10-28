use serde_json::json;
use tempfile::TempDir;
use uuid::Uuid;
use vibemux_harness::dispatch::writable_profile::WritableGrant;
use vibemux_worker_tools::{
    contract::*,
    protocol::{Protocol, ProtocolAction, ToolSurface},
    test_session::*,
};
use vibemux_workflow::PathPattern;

fn fixture() -> (TempDir, TestSession) {
    let root = TempDir::new().unwrap();
    #[cfg(windows)]
    vibemux_platform::secure_user_directory(root.path()).unwrap();
    let session = TestSession::create(
        root.path(),
        TestBootstrap {
            schema_version: 2,
            worker: Bootstrap {
                schema_version: 1,
                attempt_id: Uuid::new_v4(),
                grants: WritableGrant {
                    read_files: true,
                    write_files: true,
                    edit_files: true,
                    run_dev_tests: true,
                    ..WritableGrant::NONE
                },
                owned_paths: vec![PathPattern::new("src/**").unwrap()],
                read_scopes: vec![],
                forbidden_paths: vec![],
                files: vec![TextFile {
                    path: "src/test.cjs".into(),
                    content: "process.exit(0);".into(),
                }],
            },
            test_policy: TestPolicy {
                backend: root.path().join("missing_codex.exe"),
                node: root.path().join("missing_node.exe"),
                suites: vec![TestSuite {
                    suite_id: "unit".into(),
                    test_entries: vec!["src/test.cjs".into()],
                    timeout_ms: 1000,
                    output_limit_bytes: 1024,
                }],
                max_runs: 2,
            },
        },
        &root.path().join("missing_trampoline.exe"),
    )
    .unwrap();
    (root, session)
}

#[test]
fn version_two_catalog_exposes_owner_selected_test_tool() {
    let (_root, mut session) = fixture();
    let mut protocol = Protocol::default();
    protocol.handle(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}), &mut session.tools).unwrap();
    protocol
        .handle(
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            &mut session.tools,
        )
        .unwrap();
    let surface = ToolSurface {
        grants: session.tools.grants(),
        test_suites: session.suites(),
    };
    let ProtocolAction::Reply(reply) = protocol
        .accept(
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
            &surface,
            false,
        )
        .unwrap()
    else {
        panic!("catalog reply")
    };
    assert!(
        reply["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "run_dev_tests")
    );
}

#[tokio::test]
async fn lost_reply_cannot_replay_same_snapshot_even_after_noop_edit() {
    let (_root, session) = fixture();
    let (_owner, cancellation) = tokio::sync::watch::channel(false);
    let (mut session, first) = session
        .run(
            TestCall {
                suite_id: "unit".into(),
                expected_revision: 0,
            },
            cancellation.clone(),
        )
        .await
        .unwrap();
    assert_eq!(first.unwrap()["status"], "refused"); // missing fixed runtime; no process runs
    assert_eq!(session.receipts().len(), 1);
    session
        .tools
        .execute(Operation::Edit {
            path: "src/test.cjs".into(),
            expected_revision: 0,
            content: "process.exit(0);".into(),
        })
        .unwrap();
    let revision = session.tools.revision();
    assert!(revision > 0);
    let (session, repeated) = session
        .run(
            TestCall {
                suite_id: "unit".into(),
                expected_revision: revision,
            },
            cancellation,
        )
        .await
        .unwrap();
    assert_eq!(repeated.unwrap_err(), ToolError::ExecutionAlreadyAdmitted);
    assert_eq!(session.receipts().len(), 1);
}

#[tokio::test]
async fn stale_revision_does_not_admit_a_test_or_mutate_candidate() {
    let (_root, session) = fixture();
    let (_owner, cancellation) = tokio::sync::watch::channel(false);
    let (mut session, result) = session
        .run(
            TestCall {
                suite_id: "unit".into(),
                expected_revision: 9,
            },
            cancellation,
        )
        .await
        .unwrap();
    assert_eq!(result.unwrap_err(), ToolError::StaleRevision);
    assert!(session.receipts().is_empty());
    assert!(session.tools.candidate().unwrap().changed_files.is_empty());
}

#[test]
fn busy_calls_are_refused_and_cancellation_keeps_request_id_types() {
    let (_root, mut session) = fixture();
    let mut protocol = Protocol::default();
    protocol.handle(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}), &mut session.tools).unwrap();
    protocol
        .handle(
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            &mut session.tools,
        )
        .unwrap();
    let surface = ToolSurface {
        grants: session.tools.grants(),
        test_suites: session.suites(),
    };
    let call = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"read_file","arguments":{"path":"src/test.cjs"}}});
    let ProtocolAction::Reply(reply) = protocol.accept(call.clone(), &surface, true).unwrap()
    else {
        panic!("busy reply")
    };
    assert_eq!(reply["error"]["message"], "worker_tools_busy");
    let ProtocolAction::Reply(reply) = protocol.accept(call, &surface, false).unwrap() else {
        panic!("duplicate reply")
    };
    assert_eq!(reply["error"]["message"], "duplicate_request_id");
    for id in [json!(2), json!("2")] {
        let ProtocolAction::Cancel { id: parsed } = protocol.accept(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":id}}), &surface, true).unwrap() else {panic!("cancellation")};
        assert_eq!(parsed, id);
    }
    for params in [
        json!({"requestId":null}),
        json!({"requestId":2,"unexpected":true}),
        json!({"requestId":2,"reason":"x".repeat(257)}),
    ] {
        assert!(matches!(
            protocol
                .accept(
                    json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":params}),
                    &surface,
                    true
                )
                .unwrap(),
            ProtocolAction::None
        ));
    }
    assert!(session.tools.candidate().unwrap().changed_files.is_empty());
}

#[test]
fn removing_either_grant_hides_tests_and_refuses_forged_calls() {
    let (_root, mut session) = fixture();
    for (read, run) in [(false, true), (true, false)] {
        let mut protocol = Protocol::default();
        protocol.handle(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}), &mut session.tools).unwrap();
        protocol
            .handle(
                json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                &mut session.tools,
            )
            .unwrap();
        let mut grants = session.tools.grants();
        grants.read_files = read;
        grants.run_dev_tests = run;
        let surface = ToolSurface {
            grants,
            test_suites: session.suites(),
        };
        let ProtocolAction::Reply(reply) = protocol
            .accept(
                json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
                &surface,
                false,
            )
            .unwrap()
        else {
            panic!("list")
        };
        assert!(!reply.to_string().contains("run_dev_tests"));
        let ProtocolAction::Reply(reply)=protocol.accept(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"run_dev_tests","arguments":{"suite_id":"unit","expected_revision":0}}}),&surface,false).unwrap() else {panic!("deny")};
        assert_eq!(reply["result"]["isError"], true);
    }
}

#[test]
fn dropping_an_unpolled_test_keeps_the_only_edited_candidate() {
    let (_root, mut session) = fixture();
    session
        .tools
        .execute(Operation::Edit {
            path: "src/test.cjs".into(),
            expected_revision: 0,
            content: "edited candidate".into(),
        })
        .unwrap();
    let retained = session.recovery_hint().retained_directory;
    let (_owner, cancellation) = tokio::sync::watch::channel(false);
    let revision = session.tools.revision();
    drop(session.run(
        TestCall {
            suite_id: "unit".into(),
            expected_revision: revision,
        },
        cancellation,
    ));
    assert_eq!(
        std::fs::read_to_string(retained.join("src/test.cjs")).unwrap(),
        "edited candidate"
    );
}

#[tokio::test]
async fn dropping_an_idle_server_keeps_edits_until_owner_publication() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (_root, session) = fixture();
    let retained = session.recovery_hint().retained_directory;
    let (server, client) = tokio::io::duplex(65536);
    let (mut input, mut output) = tokio::io::split(server);
    let (reader, mut writer) = tokio::io::split(client);
    let mut reader = BufReader::new(reader);
    let mut serving = Box::pin(vibemux_worker_tools::test_server::serve_tests(
        &mut input,
        &mut output,
        session,
    ));
    let edit = async {
        for message in [
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"retention","version":"1"}}}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"edit_file","arguments":{"path":"src/test.cjs","expected_revision":0,"content":"edited while serving"}}}),
        ] {
            writer
                .write_all(format!("{message}\n").as_bytes())
                .await
                .unwrap();
        }
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        line.clear();
        reader.read_line(&mut line).await.unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&line).unwrap()["result"]["isError"],
            false
        );
    };
    tokio::select! { _ = &mut serving => panic!("server ended before owner publication"), () = edit => () }
    drop(serving);
    assert_eq!(
        std::fs::read_to_string(retained.join("src/test.cjs")).unwrap(),
        "edited while serving"
    );
}

#[test]
fn only_successful_candidate_publication_permits_cleanup() {
    use vibemux_worker_tools::{
        owner_output::finalize_test_session, test_server::TestTransportOutcome,
    };
    for (transport_ok, sink_ok) in [(false, true), (true, false), (true, true)] {
        let (_root, mut session) = fixture();
        session
            .tools
            .execute(Operation::Edit {
                path: "src/test.cjs".into(),
                expected_revision: 0,
                content: "latest candidate".into(),
            })
            .unwrap();
        let recovery_hint = session.recovery_hint();
        let retained = recovery_hint.retained_directory.clone();
        let mut published = Vec::new();
        let result = finalize_test_session(
            TestTransportOutcome {
                session: Some(session),
                recovery_hint,
                transport: if transport_ok {
                    Ok(())
                } else {
                    Err("fixture_transport_error")
                },
            },
            |bytes| {
                if sink_ok {
                    published.extend_from_slice(bytes);
                    Ok(())
                } else {
                    published.extend_from_slice(&bytes[..bytes.len() / 2]);
                    Err(std::io::Error::other("uncertain fixture publication"))
                }
            },
        );
        if transport_ok && sink_ok {
            assert!(result.is_ok());
            assert!(!retained.exists());
            let candidate: serde_json::Value = serde_json::from_slice(&published).unwrap();
            assert_eq!(candidate["status"], "candidate");
            assert_eq!(
                candidate["candidate"]["changed_files"][0]["content"],
                "latest candidate"
            );
        } else {
            assert!(result.is_err());
            assert_eq!(
                std::fs::read_to_string(retained.join("src/test.cjs")).unwrap(),
                "latest candidate"
            );
            if sink_ok {
                let recovery: serde_json::Value = serde_json::from_slice(&published).unwrap();
                assert_eq!(recovery["status"], "outcome_unknown");
            }
        }
    }
}

#[test]
fn publisher_panic_cannot_authorize_candidate_deletion() {
    use vibemux_worker_tools::{
        owner_output::finalize_test_session, test_server::TestTransportOutcome,
    };
    let (_root, mut session) = fixture();
    session
        .tools
        .execute(Operation::Edit {
            path: "src/test.cjs".into(),
            expected_revision: 0,
            content: "preserve on unwind".into(),
        })
        .unwrap();
    let recovery_hint = session.recovery_hint();
    let retained = recovery_hint.retained_directory.clone();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = finalize_test_session(
            TestTransportOutcome {
                session: Some(session),
                recovery_hint,
                transport: Ok(()),
            },
            |_bytes| panic!("fixture sink panic"),
        );
    }));
    assert!(result.is_err());
    assert_eq!(
        std::fs::read_to_string(retained.join("src/test.cjs")).unwrap(),
        "preserve on unwind"
    );
}
