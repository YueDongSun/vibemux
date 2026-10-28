use uuid::Uuid;
use vibemux_harness::dispatch::writable_profile::WritableGrant;
use vibemux_worker_tools::{
    contract::{TextFile, ToolError},
    dev_tests::{DEV_TEST_SCHEMA, DevTestRequest, MAX_OUTPUT_BYTES, MAX_TIMEOUT_MS},
};
use vibemux_workflow::PathPattern;

fn request() -> DevTestRequest {
    DevTestRequest {
        schema_version: DEV_TEST_SCHEMA,
        attempt_id: Uuid::new_v4(),
        operation_id: Uuid::new_v4(),
        suite_id: "node_unit".into(),
        grants: WritableGrant {
            read_files: true,
            run_dev_tests: true,
            ..WritableGrant::NONE
        },
        read_scopes: vec![PathPattern::new("src/**").unwrap()],
        forbidden_paths: vec![],
        files: vec![TextFile {
            path: "src/example.test.cjs".into(),
            content: "require('node:test')('example',()=>{});".into(),
        }],
        test_entries: vec!["src/example.test.cjs".into()],
        timeout_ms: 1000,
        output_limit_bytes: 4096,
    }
}

#[test]
fn execution_and_disclosure_grants_are_both_required() {
    assert_eq!(request().validate(), Ok(()));
    for (read, run) in [(false, false), (false, true), (true, false)] {
        let mut input = request();
        input.grants.read_files = read;
        input.grants.run_dev_tests = run;
        assert_eq!(input.validate(), Err(ToolError::Denied));
    }
}

#[test]
fn every_executable_input_requires_read_scope_even_if_it_is_not_a_test_entry() {
    let mut input = request();
    input.files.push(TextFile {
        path: "private/context.txt".into(),
        content: "private".into(),
    });
    assert_eq!(input.validate(), Err(ToolError::OutsideScope));
    input
        .read_scopes
        .push(PathPattern::new("private/**").unwrap());
    input
        .forbidden_paths
        .push(PathPattern::new("private/context.txt").unwrap());
    assert_eq!(input.validate(), Err(ToolError::ProtectedPath));
}

#[test]
fn entries_are_explicit_existing_files_and_limits_are_owner_bounded() {
    for entry in [
        "src/missing.cjs",
        "--require",
        "../outside.cjs",
        "src/*.cjs",
        "src",
    ] {
        let mut input = request();
        input.test_entries = vec![entry.into()];
        assert!(input.validate().is_err(), "{entry}");
    }
    let mut input = request();
    input.test_entries.clear();
    assert!(input.validate().is_err());
    let mut input = request();
    input.test_entries.push(input.test_entries[0].clone());
    assert!(input.validate().is_err());
    for duration in [0, MAX_TIMEOUT_MS + 1] {
        let mut input = request();
        input.timeout_ms = duration;
        assert!(input.validate().is_err());
    }
    for limit in [0, MAX_OUTPUT_BYTES + 1] {
        let mut input = request();
        input.output_limit_bytes = limit;
        assert!(input.validate().is_err());
    }
    let mut value = serde_json::to_value(request()).unwrap();
    value["arguments"] = serde_json::json!(["--require", "untrusted"]);
    assert!(serde_json::from_value::<DevTestRequest>(value).is_err());
}
