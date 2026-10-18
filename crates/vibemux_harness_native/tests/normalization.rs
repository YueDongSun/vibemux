use serde_json::{Value, json};
use uuid::Uuid;
use vibemux_harness_native::contract::{
    Completion, Harness, Kind, Outcome, Protocol, Record, Request, SCHEMA_VERSION, Status,
};
use vibemux_harness_native::normalize::classify;

#[tokio::test]
async fn fragmented_utf8_frame_survives_a_cancelled_read_future() {
    use std::time::Duration;
    use tokio::io::{AsyncWriteExt, BufReader};
    use vibemux_harness_native::json_lines::Reader;

    let (mut writer, input) = tokio::io::duplex(128);
    let mut reader = Reader::new(BufReader::with_capacity(2, input), 128).unwrap();
    writer.write_all(b"{\"text\":\"\xe4").await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(25), reader.next())
            .await
            .is_err()
    );
    writer
        .write_all(b"\xbd\xa0\xe5\xa5\xbd\"}\r\n")
        .await
        .unwrap();
    writer.shutdown().await.unwrap();
    assert_eq!(
        reader.next().await.unwrap().unwrap(),
        "{\"text\":\"\u{4f60}\u{597d}\"}"
    );
    assert!(reader.next().await.unwrap().is_none());
}

#[tokio::test]
async fn frame_and_trace_limits_reject_without_silent_truncation() {
    use tokio::io::BufReader;
    use vibemux_harness_native::{
        config::Limits,
        json_lines::{Reader, replay},
    };

    let mut oversized = Reader::new(BufReader::new(&b"{\"value\":12345}"[..]), 8).unwrap();
    assert_eq!(
        oversized.next().await.unwrap_err().code(),
        "harness_frame_too_large"
    );
    let mut invalid = Reader::new(BufReader::new(&b"\xff\n"[..]), 8).unwrap();
    assert_eq!(
        invalid.next().await.unwrap_err().code(),
        "harness_invalid_utf8"
    );
    let mut blank = Reader::new(BufReader::new(&b"\n"[..]), 8).unwrap();
    assert_eq!(
        blank.next().await.unwrap_err().code(),
        "harness_empty_record"
    );
    let mut final_line = Reader::new(BufReader::new(&b"{}"[..]), 8).unwrap();
    assert_eq!(final_line.next().await.unwrap().as_deref(), Some("{}"));
    let limits = Limits {
        record_count: 1,
        ..Limits::default()
    };
    let mut input = BufReader::new(&b"{}\n{}\n"[..]);
    assert_eq!(
        replay(&mut input, Protocol::Acp, &limits)
            .await
            .unwrap_err()
            .code(),
        "harness_capture_too_large"
    );
}

const TOKEN_SENTINEL: &str = "SYNTHETIC_SECRET_TOKEN";

#[tokio::test]
async fn probe_request_identity_is_validated_before_any_native_io() {
    use std::path::PathBuf;
    use tokio::sync::{mpsc, watch};
    use vibemux_harness_native::{
        config::{Limits, Route},
        native,
    };

    let route = Route {
        harness: Harness::Codex,
        protocol: Protocol::Appserver,
        executable: PathBuf::from("missing_native_executable"),
        arguments: vec![],
        working_directory: PathBuf::from("missing_native_workspace"),
        environment_names: vec![],
        enabled: true,
        allow_execution: false,
        model: None,
    };
    for (schema_version, request_id) in [(99, Uuid::new_v4()), (SCHEMA_VERSION, Uuid::nil())] {
        let (records, _receiver) = mpsc::channel(1);
        let (_cancel_sender, cancellation) = watch::channel(false);
        let request = Request {
            schema_version,
            request_id,
            harness: Harness::Codex,
            prompt: String::new(),
        };
        assert_eq!(
            native::run(
                route.clone(),
                Limits::default(),
                request,
                true,
                records,
                cancellation
            )
            .await
            .unwrap_err()
            .code(),
            "harness_invalid_request"
        );
    }
}
const PATH_SENTINEL: &str = "SYNTHETIC_PRIVATE_PATH";
const SHELL_TEXT: &str = "literal $(synthetic_command); & | > < `token` %SYNTHETIC_ENV%";
const ALL_PROTOCOLS: [Protocol; 7] = [
    Protocol::Codex,
    Protocol::Appserver,
    Protocol::Claude,
    Protocol::Opencode,
    Protocol::Copilot,
    Protocol::Acp,
    Protocol::Grok,
];

fn parse_fixture(protocol: Protocol, fixture: &str, expected: &[Kind]) -> Vec<Record> {
    let records: Vec<Record> = fixture
        .lines()
        .enumerate()
        .map(|(index, raw_json)| {
            let sequence = u64::try_from(index).unwrap() + 1;
            let record = Record::parse(protocol, sequence, raw_json.to_owned()).unwrap();
            assert_eq!(record.raw_json().as_bytes(), raw_json.as_bytes());
            record
        })
        .collect();
    let observed: Vec<Kind> = records.iter().map(|record| record.kind).collect();
    assert_eq!(observed, expected);
    records
}

#[test]
fn codex_exec_fixture_keeps_all_chunks_and_extensions() {
    let records = parse_fixture(
        Protocol::Codex,
        include_str!("fixtures/codex_exec.jsonl"),
        &[
            Kind::Started,
            Kind::Started,
            Kind::Text,
            Kind::Text,
            Kind::Tool,
            Kind::Opaque,
            Kind::Completed,
        ],
    );
    assert_eq!(records[4].value().unwrap()["item"]["command"], SHELL_TEXT);
    assert_eq!(
        records[5].value().unwrap()["extension"]["nested"][2]["preserve"],
        TOKEN_SENTINEL,
    );
}

#[test]
fn appserver_fixture_classifies_item_failure_as_tool_activity() {
    parse_fixture(
        Protocol::Appserver,
        include_str!("fixtures/codex_app_server.jsonl"),
        &[
            Kind::Opaque,
            Kind::Opaque,
            Kind::Opaque,
            Kind::Started,
            Kind::Text,
            Kind::Text,
            Kind::Tool,
            Kind::Approval,
            Kind::Usage,
            Kind::Completed,
        ],
    );
}

#[test]
fn claude_fixture_distinguishes_tools_permissions_usage_and_result() {
    parse_fixture(
        Protocol::Claude,
        include_str!("fixtures/claude_stream.jsonl"),
        &[
            Kind::Opaque,
            Kind::Started,
            Kind::Tool,
            Kind::Text,
            Kind::Text,
            Kind::Usage,
            Kind::Approval,
            Kind::Tool,
            Kind::Completed,
        ],
    );
}

#[test]
fn opencode_step_finish_never_completes_request() {
    parse_fixture(
        Protocol::Opencode,
        include_str!("fixtures/opencode_json.jsonl"),
        &[
            Kind::Started,
            Kind::Text,
            Kind::Tool,
            Kind::Usage,
            Kind::Text,
            Kind::Opaque,
        ],
    );
}

#[test]
fn acp_fixture_preserves_chunks_and_unknown_notifications() {
    parse_fixture(
        Protocol::Acp,
        include_str!("fixtures/acp_stream.jsonl"),
        &[
            Kind::Opaque,
            Kind::Opaque,
            Kind::Tool,
            Kind::Text,
            Kind::Text,
            Kind::Usage,
            Kind::Approval,
            Kind::Opaque,
            Kind::Completed,
        ],
    );
}

#[test]
fn unverified_copilot_and_grok_streams_remain_opaque() {
    for protocol in [Protocol::Copilot, Protocol::Grok] {
        parse_fixture(
            protocol,
            include_str!("fixtures/unverified_stream.jsonl"),
            &[Kind::Opaque; 3],
        );
        for payload in [
            json!({"type": "turn.completed"}),
            json!({"type": "result", "subtype": "success", "is_error": false}),
            json!({"result": {"stopReason": "end_turn"}}),
        ] {
            assert_eq!(classify(protocol, &payload), Kind::Opaque);
        }
    }
}

#[test]
fn raw_json_retains_whitespace_unicode_escapes_and_unknown_fields() {
    let raw_json = r#"  { "type" : "vendor.extension", "text": "\u4f60\u597d", "unknown" : [1, {"emoji":"\ud83d\ude80","literal":"$(synthetic_command); & | > < `token`"}] }  "#;
    for protocol in ALL_PROTOCOLS {
        let record = Record::parse(protocol, 1, raw_json.to_owned()).unwrap();
        assert_eq!(record.kind, Kind::Opaque);
        assert_eq!(record.raw_json().as_bytes(), raw_json.as_bytes());
        assert_eq!(record.value().unwrap()["text"], "你好");
        assert_eq!(record.value().unwrap()["unknown"][1]["emoji"], "🚀");
        let serialized = serde_json::to_value(&record).unwrap();
        assert_eq!(serialized["raw_json"], raw_json);
    }
}

#[test]
fn unknown_discriminants_do_not_match_known_prefixes_or_nested_claims() {
    let payloads = [
        json!({"type": "turn.completed.extension", "status": "success"}),
        json!({"method": "turn/completed/extension", "params": {"turn": {"status": "completed"}}}),
        json!({"type": "vendor.extension", "extension": {"type": "turn.completed"}}),
        json!({"method": "vendor/extension", "result": {"stopReason": "end_turn"}}),
    ];
    for protocol in ALL_PROTOCOLS {
        for payload in &payloads {
            let raw_json = serde_json::to_string(payload).unwrap();
            let record = Record::parse(protocol, 1, raw_json.clone()).unwrap();
            assert_eq!(record.kind, Kind::Opaque);
            assert_eq!(record.raw_json(), raw_json);
        }
    }
}

#[test]
fn known_terminal_discriminants_require_correct_scope_and_status() {
    for (payload, expected) in [
        (json!({"type":"turn.completed"}), Kind::Completed),
        (
            json!({"type":"turn.failed","error":{"message":"synthetic failure"}}),
            Kind::Failed,
        ),
        (
            json!({"type":"error","message":"synthetic fatal failure"}),
            Kind::Failed,
        ),
        (json!({"type":"turn.cancelled"}), Kind::Opaque),
        (
            json!({"type":"item.completed","item":{"type":"error"}}),
            Kind::Opaque,
        ),
        (
            json!({"type":"item.completed","item":{"type":"command_execution","status":"failed"}}),
            Kind::Tool,
        ),
    ] {
        assert_eq!(classify(Protocol::Codex, &payload), expected);
    }
    for (status, expected) in [
        ("completed", Kind::Completed),
        ("failed", Kind::Failed),
        ("interrupted", Kind::Cancelled),
        ("inProgress", Kind::Opaque),
        ("cancelled", Kind::Opaque),
        ("future_status", Kind::Opaque),
    ] {
        let payload = json!({"method":"turn/completed","params":{"turn":{"status":status}}});
        assert_eq!(classify(Protocol::Appserver, &payload), expected);
    }
    for payload in [
        json!({"method":"turn/completed"}),
        json!({"method":"turn/completed","status":"completed"}),
        json!({"method":"turn/completed","params":{"turn":{"status":true}}}),
        json!({"method":"turn/interrupt","params":{"turnId":"synthetic_turn"}}),
        json!({"method":"error","params":{"error":{"message":"synthetic retry"}}}),
        json!({"id":4,"error":{"code":-32601,"message":"synthetic auxiliary error"}}),
    ] {
        assert_eq!(classify(Protocol::Appserver, &payload), Kind::Opaque);
    }
}

#[test]
fn acp_stop_reasons_fail_closed_and_cancel_requests_are_not_confirmation() {
    for (stop_reason, expected) in [
        ("end_turn", Kind::Completed),
        ("cancelled", Kind::Cancelled),
        ("max_tokens", Kind::Failed),
        ("max_turn_requests", Kind::Failed),
        ("refusal", Kind::Failed),
        ("future_stop", Kind::Opaque),
    ] {
        let payload = json!({"jsonrpc":"2.0","id":3,"result":{"stopReason":stop_reason}});
        assert_eq!(classify(Protocol::Acp, &payload), expected);
    }
    for payload in [
        json!({"method":"session/cancel","params":{"sessionId":"synthetic_session"}}),
        json!({"method":"vendor/extension","result":{"stopReason":"end_turn"}}),
        json!({"method":null,"result":{"stopReason":"end_turn"}}),
        json!({"result":{"stopReason":true}}),
        json!({"result":{"status":"completed"}}),
    ] {
        assert_eq!(classify(Protocol::Acp, &payload), Kind::Opaque);
    }
    let tool_failure = json!({"method":"session/update","params":{"update":{
        "sessionUpdate":"tool_call_update","status":"failed","toolCallId":"tool_one"
    }}});
    assert_eq!(classify(Protocol::Acp, &tool_failure), Kind::Tool);
}

#[test]
fn claude_result_errors_and_cancellation_override_success() {
    for (payload, expected) in [
        (
            json!({"type":"result","subtype":"success","is_error":false}),
            Kind::Completed,
        ),
        (
            json!({"type":"result","subtype":"success"}),
            Kind::Completed,
        ),
        (
            json!({"type":"result","subtype":"success","is_error":true}),
            Kind::Failed,
        ),
        (
            json!({"type":"result","subtype":"error_max_turns","is_error":false}),
            Kind::Failed,
        ),
        (
            json!({"type":"result","subtype":"error_during_execution"}),
            Kind::Failed,
        ),
        (
            json!({"type":"result","subtype":"future_result","is_error":false}),
            Kind::Opaque,
        ),
        (
            json!({"type":"result","subtype":"success","is_error":"true"}),
            Kind::Opaque,
        ),
        (
            json!({"type":"result","subtype":"success","terminal_reason":"future_reason"}),
            Kind::Opaque,
        ),
        (
            json!({"type":"result","subtype":"success","terminal_reason":"aborted_streaming"}),
            Kind::Cancelled,
        ),
        (
            json!({"type":"result","subtype":"success","terminal_reason":"aborted_tools"}),
            Kind::Cancelled,
        ),
        (
            json!({"type":"result","subtype":"success","terminal_reason":"max_turns"}),
            Kind::Failed,
        ),
        (
            json!({"type":"stream_event","event":{"type":"message_stop"}}),
            Kind::Opaque,
        ),
        (
            json!({"type":"stream_event","event":{"type":"content_block_stop"}}),
            Kind::Opaque,
        ),
        (
            json!({"type":"control_request","request":{"subtype":"interrupt"}}),
            Kind::Opaque,
        ),
        (
            json!({"type":"system","subtype":"task_notification","status":"completed"}),
            Kind::Opaque,
        ),
    ] {
        assert_eq!(classify(Protocol::Claude, &payload), expected);
    }
}

#[test]
fn opaque_and_nonobject_payloads_are_safe_without_execution() {
    for protocol in ALL_PROTOCOLS {
        for value in [
            Value::Null,
            json!(false),
            json!(7),
            json!(SHELL_TEXT),
            json!([]),
            json!({}),
        ] {
            assert_eq!(classify(protocol, &value), Kind::Opaque);
        }
    }
    let raw_json = json!({"type":"vendor.extension","text":SHELL_TEXT}).to_string();
    let record = Record::parse(Protocol::Codex, 1, raw_json.clone()).unwrap();
    assert_eq!(record.raw_json(), raw_json);
    assert_eq!(record.value().unwrap()["text"], SHELL_TEXT);
}

#[test]
fn truncated_stream_has_no_inferred_completion_record() {
    let records: Vec<Record> = include_str!("fixtures/codex_exec.jsonl")
        .lines()
        .take(4)
        .enumerate()
        .map(|(index, raw_json)| {
            Record::parse(
                Protocol::Codex,
                u64::try_from(index).unwrap() + 1,
                raw_json.to_owned(),
            )
            .unwrap()
        })
        .collect();
    assert_eq!(records.len(), 4);
    assert_eq!(records.last().unwrap().kind, Kind::Text);
    assert!(!records.iter().any(|record| matches!(
        record.kind,
        Kind::Completed | Kind::Failed | Kind::Cancelled
    )));
}

#[test]
fn public_summaries_and_debug_never_reveal_vendor_content() {
    let raw_json = json!({
        "type":"vendor.extension", "token":TOKEN_SENTINEL, "path":PATH_SENTINEL,
        "prompt":SHELL_TEXT,
    })
    .to_string();
    let record = Record::parse(Protocol::Codex, 1, raw_json.clone()).unwrap();
    let observation = serde_json::to_value(record.summary()).unwrap();
    assert_eq!(
        observation,
        json!({"sequence":1,"kind":"opaque","bytes":raw_json.len()})
    );
    let request = Request {
        schema_version: SCHEMA_VERSION,
        request_id: Uuid::from_u128(1),
        harness: Harness::Codex,
        prompt: format!("{TOKEN_SENTINEL} {PATH_SENTINEL} {SHELL_TEXT}"),
    };
    let outcome = Outcome {
        request_id: request.request_id,
        harness: request.harness,
        records: vec![record.clone()],
        completion: Completion {
            schema_version: SCHEMA_VERSION,
            status: Status::Unverified,
            record_count: 1,
            stdout_bytes: u64::try_from(raw_json.len()).unwrap(),
            stderr_bytes: 0,
            exit_code: Some(0),
            forced_termination: false,
            error_code: None,
        },
    };
    let invalid_json = format!("{{\"{TOKEN_SENTINEL}\":\"{PATH_SENTINEL}");
    let error = Record::parse(Protocol::Codex, 1, invalid_json).unwrap_err();
    for public_text in [
        format!("{record:?}"),
        format!("{:?}", record.summary()),
        observation.to_string(),
        format!("{request:?}"),
        format!("{outcome:?}"),
        outcome.summary().to_string(),
        error.to_string(),
        format!("{error:?}"),
    ] {
        for sentinel in [TOKEN_SENTINEL, PATH_SENTINEL, SHELL_TEXT] {
            assert!(!public_text.contains(sentinel));
        }
    }
    assert_eq!(outcome.records[0].raw_json(), raw_json);
}

#[test]
fn nested_claude_result_cannot_terminate_root_request() {
    for result_kind in ["success", "error_during_execution"] {
        for terminal_reason in ["completed", "aborted_tools", "aborted_streaming"] {
            for is_error in [false, true] {
                let payload = json!({
                    "type":"result", "subtype":result_kind, "is_error":is_error,
                    "terminal_reason":terminal_reason, "parent_tool_use_id":"synthetic_parent_tool",
                });
                assert_eq!(classify(Protocol::Claude, &payload), Kind::Opaque);
            }
        }
    }
    let root_result = json!({
        "type":"result", "subtype":"success", "is_error":false,
        "parent_tool_use_id":null,
    });
    assert_eq!(classify(Protocol::Claude, &root_result), Kind::Completed);
}

#[test]
fn opencode_discriminants_are_exact_and_none_completes_a_request() {
    let cases = [
        (
            json!({"type":"step_start","part":{"type":"step-start"}}),
            Kind::Started,
        ),
        (json!({"type":"text","part":{"text":"chunk"}}), Kind::Text),
        (
            json!({"type":"reasoning","part":{"text":"thought"}}),
            Kind::Text,
        ),
        (
            json!({"type":"tool_use","part":{"tool":"synthetic_tool"}}),
            Kind::Tool,
        ),
        (
            json!({"type":"error","error":{"name":"synthetic_error"}}),
            Kind::Failed,
        ),
        (
            json!({"type":"step_finish","part":{"tokens":{"input":1}}}),
            Kind::Usage,
        ),
        (
            json!({"type":"step_finish","part":{"tokens":{}}}),
            Kind::Usage,
        ),
        (json!({"type":"step_finish","part":{"cost":0}}), Kind::Usage),
        (
            json!({"type":"step_finish","part":{"cost":0.25}}),
            Kind::Usage,
        ),
        // A step ending without usage data, or with malformed data, is opaque.
        (
            json!({"type":"step_finish","part":{"reason":"stop"}}),
            Kind::Opaque,
        ),
        (
            json!({"type":"step_finish","part":{"tokens":7,"cost":"0.25"}}),
            Kind::Opaque,
        ),
        (
            json!({"type":"step_finish","part":{"tokens":null,"cost":null}}),
            Kind::Opaque,
        ),
        (
            json!({"type":"step_finish","tokens":{"input":1},"cost":0}),
            Kind::Opaque,
        ),
        // Discriminants match exactly.
        (json!({"type":"Step_Start"}), Kind::Opaque),
        (json!({"type":"text "}), Kind::Opaque),
        (json!({"type":"tool"}), Kind::Opaque),
        (json!({"type":["error"]}), Kind::Opaque),
        (json!({"part":{"type":"text"}}), Kind::Opaque),
        // Names that sound final still never complete a request.
        (
            json!({"type":"step_finish","part":{"reason":"end_turn"}}),
            Kind::Opaque,
        ),
        (json!({"type":"session.idle"}), Kind::Opaque),
        (json!({"type":"done"}), Kind::Opaque),
    ];
    for (payload, expected) in cases {
        assert_eq!(
            classify(Protocol::Opencode, &payload),
            expected,
            "{payload}"
        );
    }
    let guide = include_str!("../../../docs/guides/harness_core_guide.md");
    assert!(guide.contains("No `opencode_json` record reads as a completion"));
}
