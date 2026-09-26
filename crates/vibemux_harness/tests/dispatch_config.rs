//! Route config, request, fingerprint, and launch-spec contracts.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use uuid::Uuid;
use vibemux_harness::{
    AgentKind,
    dispatch::{
        DispatchConfig, DispatchError, DispatchLimits, DispatchRequest, DispatchRoute,
        NativeProtocol, Sha256Digest,
        launch_spec::{SYSTEM_ENVIRONMENT_NAMES, SessionMode, build_launch_spec},
        request::{DISPATCH_REQUEST_SCHEMA_VERSION, MAX_PROMPT_BYTES},
        route_config::{
            DispatchCatalogEntry, ExecutablePolicy, MAX_EXECUTABLE_PATH_BYTES,
            MAX_ROUTE_CONFIG_BYTES, validate_executable,
        },
    },
};
use vibemux_types::ProjectId;

const PRIVATE_SENTINEL: &str = "SYNTHETIC_PRIVATE";
const PROMPT: &str = "synthetic prompt $(synthetic_command) & | > <";

fn absolute(name: &str) -> String {
    if cfg!(windows) {
        format!(r"C:\tools\{name}")
    } else {
        format!("/opt/tools/{name}")
    }
}

fn native_executable(name: &str) -> String {
    if cfg!(windows) {
        absolute(&format!("{name}.exe"))
    } else {
        absolute(name)
    }
}

fn route_json(harness: &str, protocol: &str) -> Value {
    json!({
        "harness": harness,
        "protocol": protocol,
        "executable": native_executable(&format!("{PRIVATE_SENTINEL}_{harness}")),
        "environment_names": [format!("{PRIVATE_SENTINEL}_KEY")],
        "enabled": true,
        "allow_execution": true,
    })
}

fn config_json(routes: Vec<Value>) -> Value {
    json!({ "schema_version": 1, "routes": routes })
}

fn parse(config: &Value) -> Result<DispatchConfig, DispatchError> {
    let bytes = serde_json::to_vec(config).expect("config bytes");
    DispatchConfig::parse(&bytes, ExecutablePolicy::for_host()).map(|pinned| pinned.config)
}

fn parse_route(route: Value) -> Result<DispatchConfig, DispatchError> {
    parse(&config_json(vec![route]))
}

fn with(mut route: Value, key: &str, value: Value) -> Value {
    route[key] = value;
    route
}

fn route(harness: AgentKind, protocol: NativeProtocol, model: Option<&str>) -> DispatchRoute {
    let mut value = route_json(harness.command_name(), protocol.as_str());
    value["harness"] = serde_json::to_value(harness).expect("harness");
    if let Some(model) = model {
        value["model"] = json!(model);
    }
    parse_route(value).expect("route").routes.remove(0)
}

fn request(harness: AgentKind, prompt: &str) -> DispatchRequest {
    DispatchRequest {
        schema_version: DISPATCH_REQUEST_SCHEMA_VERSION,
        request_id: Uuid::from_u128(0x1234),
        harness,
        prompt: prompt.to_string(),
    }
}

#[test]
fn a_valid_config_parses_and_pins_the_digest_of_the_exact_bytes() {
    let config = config_json(vec![
        route_json("codex", "codex_app_server"),
        route_json("claude", "claude_stream_json"),
        route_json("open_code", "acp"),
        route_json("copilot", "acp"),
        route_json("grok", "acp"),
    ]);
    let bytes = serde_json::to_vec_pretty(&config).unwrap();
    let pinned = DispatchConfig::parse(&bytes, ExecutablePolicy::for_host()).expect("config");
    assert_eq!(pinned.digest, Sha256Digest::of(&bytes));
    assert_eq!(pinned.config.routes.len(), 5);
    assert_eq!(pinned.config.limits, DispatchLimits::default());

    let compact = serde_json::to_vec(&config).unwrap();
    let repinned = DispatchConfig::parse(&compact, ExecutablePolicy::for_host()).unwrap();
    assert_ne!(
        pinned.digest, repinned.digest,
        "digest covers bytes, not meaning"
    );
}

#[test]
fn config_shape_violations_are_rejected_with_fixed_codes() {
    let oversized = vec![b' '; MAX_ROUTE_CONFIG_BYTES + 1];
    assert_eq!(
        DispatchConfig::parse(&oversized, ExecutablePolicy::for_host()).unwrap_err(),
        DispatchError::ConfigTooLarge
    );
    let invalid = |config: Value| parse(&config).unwrap_err();
    assert_eq!(
        DispatchConfig::parse(b"{", ExecutablePolicy::for_host()).unwrap_err(),
        DispatchError::ConfigInvalid
    );
    let mut unknown = config_json(vec![route_json("codex", "codex_exec")]);
    unknown["extra"] = json!(true);
    assert_eq!(invalid(unknown), DispatchError::ConfigInvalid);
    let mut version = config_json(vec![route_json("codex", "codex_exec")]);
    version["schema_version"] = json!(2);
    assert_eq!(invalid(version), DispatchError::ConfigInvalid);
    assert_eq!(invalid(config_json(vec![])), DispatchError::ConfigInvalid);
    assert_eq!(
        invalid(config_json(vec![route_json("codex", "codex_exec"); 17])),
        DispatchError::ConfigInvalid
    );
    assert_eq!(
        invalid(config_json(vec![
            route_json("codex", "codex_exec"),
            route_json("codex", "codex_app_server"),
        ])),
        DispatchError::ConfigRouteInvalid
    );
    // Schema 1 carries no operator argv; the field is unknown, not ignored.
    assert_eq!(
        parse_route(with(
            route_json("codex", "codex_exec"),
            "arguments",
            json!(["--yolo"])
        ))
        .unwrap_err(),
        DispatchError::ConfigInvalid
    );
    let mut working_directory = route_json("codex", "codex_exec");
    working_directory["working_directory"] = json!(absolute("elsewhere"));
    assert_eq!(
        parse_route(working_directory).unwrap_err(),
        DispatchError::ConfigInvalid
    );
}

#[test]
fn a_protocol_must_be_a_verified_adapter_for_its_harness() {
    for harness in AgentKind::all() {
        for protocol in NativeProtocol::ALL {
            let name = serde_json::to_value(harness).unwrap();
            let result = parse_route(with(
                route_json("placeholder", protocol.as_str()),
                "harness",
                name,
            ));
            if protocol.accepts(harness) {
                assert!(result.is_ok(), "{harness:?} {protocol:?}");
            } else {
                assert_eq!(
                    result.unwrap_err(),
                    DispatchError::ConfigRouteInvalid,
                    "{harness:?} {protocol:?}"
                );
            }
        }
    }
    let accepted: Vec<_> = AgentKind::all()
        .into_iter()
        .filter(|harness| {
            NativeProtocol::ALL
                .iter()
                .any(|protocol| protocol.accepts(*harness))
        })
        .collect();
    assert_eq!(
        accepted,
        [
            AgentKind::Claude,
            AgentKind::Codex,
            AgentKind::OpenCode,
            AgentKind::Copilot,
            AgentKind::Grok,
        ]
    );
}

#[test]
fn executables_must_be_absolute_native_images_without_shims() {
    let windows = ExecutablePolicy::WindowsNativeImage;
    let posix = ExecutablePolicy::PosixExecutable;
    let check = |path: &str, policy| validate_executable(Path::new(path), policy);
    assert_eq!(check(&absolute("codex.exe"), windows), Ok(()));
    assert_eq!(check(&absolute("CODEX.EXE"), windows), Ok(()));
    assert_eq!(check(&absolute("codex"), posix), Ok(()));
    for shim in ["codex.cmd", "codex.bat", "codex.ps1", "codex", "codex.js"] {
        assert_eq!(
            check(&absolute(shim), windows),
            Err(DispatchError::ConfigExecutableInvalid),
            "{shim}"
        );
    }
    for shim in [
        "codex.cmd",
        "codex.BAT",
        "codex.ps1",
        "codex.psm1",
        "codex.vbs",
        "codex.js",
        "codex.com",
    ] {
        assert_eq!(
            check(&absolute(shim), posix),
            Err(DispatchError::ConfigExecutableInvalid),
            "{shim}"
        );
    }
    for policy in [windows, posix] {
        for path in ["", "codex.exe", r"tools\codex.exe", "tools/codex.exe"] {
            assert_eq!(
                check(path, policy),
                Err(DispatchError::ConfigExecutableInvalid),
                "{path:?}"
            );
        }
        let nul = absolute("co\0dex.exe");
        assert_eq!(
            check(&nul, policy),
            Err(DispatchError::ConfigExecutableInvalid)
        );
        let long = absolute(&format!("{}.exe", "x".repeat(MAX_EXECUTABLE_PATH_BYTES)));
        assert_eq!(
            check(&long, policy),
            Err(DispatchError::ConfigExecutableInvalid)
        );
    }
    assert_eq!(
        parse_route(with(
            route_json("codex", "codex_exec"),
            "executable",
            json!(absolute("codex.cmd"))
        ))
        .unwrap_err(),
        DispatchError::ConfigExecutableInvalid
    );
}

#[test]
fn environment_names_are_upper_snake_case_unique_and_not_reserved() {
    let names = |names: Value| {
        parse_route(with(
            route_json("codex", "codex_exec"),
            "environment_names",
            names,
        ))
    };
    assert!(names(json!(["OPENAI_API_KEY", "PATH", "HOME2", "_X"])).is_ok());
    let too_many: Vec<String> = (0..25).map(|index| format!("NAME_{index}")).collect();
    for invalid in [
        json!([""]),
        json!(["lower_case"]),
        json!(["WITH-HYPHEN"]),
        json!(["WITH SPACE"]),
        json!(["2FA_KEY"]),
        json!(["A=B"]),
        json!(["PATH", "PATH"]),
        json!(["VIBEMUX_TOKEN"]),
        json!(["X".repeat(129)]),
        json!(too_many),
    ] {
        assert_eq!(
            names(invalid.clone()).unwrap_err(),
            DispatchError::ConfigEnvironmentInvalid,
            "{invalid}"
        );
    }
}

#[test]
fn models_are_bounded_tokens_and_acp_routes_refuse_them() {
    let model = |protocol: &str, harness: &str, model: Value| {
        parse_route(with(route_json(harness, protocol), "model", model))
    };
    for valid in ["gpt-5.1-codex", "claude-opus-5-5", "org/model:tag_1"] {
        assert!(
            model("codex_exec", "codex", json!(valid)).is_ok(),
            "{valid}"
        );
        assert!(
            model("claude_stream_json", "claude", json!(valid)).is_ok(),
            "{valid}"
        );
    }
    for invalid in [
        json!(""),
        json!("-m"),
        json!("--yolo"),
        json!("a b"),
        json!("a;b"),
        json!("x".repeat(257)),
    ] {
        assert_eq!(
            model("codex_app_server", "codex", invalid.clone()).unwrap_err(),
            DispatchError::ConfigRouteInvalid,
            "{invalid}"
        );
    }
    assert_eq!(
        model("acp", "grok", json!("grok-4")).unwrap_err(),
        DispatchError::ConfigRouteInvalid
    );
}

#[test]
fn limits_default_when_omitted_and_are_range_checked() {
    let with_limits = |limits: Value| {
        let mut config = config_json(vec![route_json("codex", "codex_exec")]);
        config["limits"] = limits;
        parse(&config)
    };
    let partial = with_limits(json!({ "deadline_ms": 1_000 })).expect("partial limits");
    assert_eq!(partial.limits.deadline_ms, 1_000);
    assert_eq!(
        partial.limits.frame_bytes,
        DispatchLimits::default().frame_bytes
    );
    assert!(with_limits(json!({ "frame_bytes": 64, "capture_bytes": 64 })).is_ok());
    assert!(with_limits(json!({ "deadline_ms": 3_600_000, "shutdown_grace_ms": 10_000 })).is_ok());
    for invalid in [
        json!({ "frame_bytes": 63 }),
        json!({ "frame_bytes": 128 * 1024 + 1 }),
        json!({ "frame_bytes": 1024, "capture_bytes": 1023 }),
        json!({ "capture_bytes": 16 * 1024 * 1024 + 1 }),
        json!({ "record_count": 0 }),
        json!({ "record_count": 16_385 }),
        json!({ "deadline_ms": 999 }),
        json!({ "deadline_ms": 3_600_001 }),
        json!({ "shutdown_grace_ms": 99 }),
        json!({ "shutdown_grace_ms": 10_001 }),
        json!({ "unknown": 1 }),
    ] {
        assert_eq!(
            with_limits(invalid.clone()).unwrap_err(),
            DispatchError::ConfigInvalid,
            "{invalid}"
        );
    }
}

#[test]
fn execution_gates_run_in_a_fixed_order_before_any_state_change() {
    let config = parse(&config_json(vec![
        route_json("codex", "codex_exec"),
        with(
            route_json("claude", "claude_stream_json"),
            "enabled",
            json!(false),
        ),
        with(route_json("grok", "acp"), "allow_execution", json!(false)),
    ]))
    .unwrap();
    let codex = request(AgentKind::Codex, PROMPT);
    assert_eq!(
        config.execution_route(&codex, true).unwrap().harness,
        AgentKind::Codex
    );
    assert_eq!(
        config.execution_route(&codex, false).unwrap_err(),
        DispatchError::NotDetected
    );
    assert_eq!(
        config
            .execution_route(&request(AgentKind::Grok, PROMPT), false)
            .unwrap_err(),
        DispatchError::ExecutionDisabled
    );
    for missing in [AgentKind::Claude, AgentKind::Qwen] {
        assert_eq!(
            config
                .execution_route(&request(missing, PROMPT), false)
                .unwrap_err(),
            DispatchError::RouteUnavailable
        );
    }
    assert_eq!(
        config
            .execution_route(&request(AgentKind::Qwen, " \n"), false)
            .unwrap_err(),
        DispatchError::InvalidPrompt
    );
    let mut nil = request(AgentKind::Qwen, "");
    nil.request_id = Uuid::nil();
    assert_eq!(
        config.execution_route(&nil, false).unwrap_err(),
        DispatchError::InvalidRequest
    );
}

#[test]
fn probe_gates_skip_execution_consent_but_not_detection() {
    let config = parse(&config_json(vec![
        route_json("codex", "codex_exec"),
        with(route_json("grok", "acp"), "allow_execution", json!(false)),
        with(
            route_json("claude", "claude_stream_json"),
            "enabled",
            json!(false),
        ),
    ]))
    .unwrap();
    assert_eq!(
        config.probe_route(AgentKind::Grok, true).unwrap().harness,
        AgentKind::Grok
    );
    assert_eq!(
        config.probe_route(AgentKind::Grok, false).unwrap_err(),
        DispatchError::NotDetected
    );
    assert_eq!(
        config.probe_route(AgentKind::Codex, true).unwrap_err(),
        DispatchError::ProbeUnsupported
    );
    assert_eq!(
        config.probe_route(AgentKind::Claude, true).unwrap_err(),
        DispatchError::RouteUnavailable
    );
}

#[test]
fn catalog_and_debug_output_carry_no_paths_or_environment_names() {
    let config = parse(&config_json(vec![
        route_json("codex", "codex_exec"),
        with(route_json("grok", "acp"), "allow_execution", json!(false)),
    ]))
    .unwrap();
    let catalog = config.catalog();
    assert_eq!(
        catalog,
        [
            DispatchCatalogEntry {
                harness: AgentKind::Codex,
                protocol: NativeProtocol::CodexExec,
                enabled: true,
                allow_execution: true,
                probe_supported: false,
            },
            DispatchCatalogEntry {
                harness: AgentKind::Grok,
                protocol: NativeProtocol::Acp,
                enabled: true,
                allow_execution: false,
                probe_supported: true,
            },
        ]
    );
    let serialized = serde_json::to_string(&catalog).unwrap();
    let debug = format!("{config:?}");
    for text in [&serialized, &debug] {
        assert!(!text.contains(PRIVATE_SENTINEL), "{text}");
        assert!(!text.contains("tools"), "{text}");
    }
    let spec = build_launch_spec(&config.routes[0], SessionMode::Execute).unwrap();
    assert!(!format!("{spec:?}").contains(PRIVATE_SENTINEL));
}

#[test]
fn requests_validate_their_shape_and_debug_hides_the_prompt() {
    assert_eq!(request(AgentKind::Codex, PROMPT).validate(), Ok(()));
    let at_limit = "x".repeat(MAX_PROMPT_BYTES);
    assert_eq!(request(AgentKind::Codex, &at_limit).validate(), Ok(()));
    let over_limit = "x".repeat(MAX_PROMPT_BYTES + 1);
    assert_eq!(
        request(AgentKind::Codex, &over_limit).validate(),
        Err(DispatchError::InvalidPrompt)
    );
    assert_eq!(
        request(AgentKind::Codex, "\t \r\n").validate(),
        Err(DispatchError::InvalidPrompt)
    );
    let mut version = request(AgentKind::Codex, PROMPT);
    version.schema_version = 2;
    assert_eq!(version.validate(), Err(DispatchError::InvalidRequest));

    let parsed: DispatchRequest = serde_json::from_value(json!({
        "schema_version": 1,
        "request_id": "00000000-0000-0000-0000-000000001234",
        "harness": "open_code",
        "prompt": PROMPT,
    }))
    .expect("request");
    assert_eq!(parsed.harness, AgentKind::OpenCode);
    for extra in ["executable", "arguments", "working_directory", "method"] {
        let mut value = json!({
            "schema_version": 1,
            "request_id": "00000000-0000-0000-0000-000000001234",
            "harness": "codex",
            "prompt": PROMPT,
        });
        value[extra] = json!("x");
        assert!(
            serde_json::from_value::<DispatchRequest>(value).is_err(),
            "{extra}"
        );
    }
    let debug = format!("{parsed:?}");
    assert!(!debug.contains("synthetic prompt"), "{debug}");
    assert!(debug.contains(&format!("prompt_bytes: {}", PROMPT.len())));
    let digest = parsed.prompt_digest();
    assert_eq!(digest.sha256, Sha256Digest::of(PROMPT.as_bytes()));
    assert_eq!(digest.byte_count, PROMPT.len() as u64);
}

#[test]
fn the_fingerprint_changes_with_every_input_and_nothing_else() {
    let project = ProjectId::from_uuid(Uuid::from_u128(1)).unwrap();
    let config = Sha256Digest::of(b"config");
    let base = request(AgentKind::Codex, PROMPT);
    let fingerprint = base.fingerprint(project, NativeProtocol::CodexExec, config);
    assert_eq!(
        fingerprint,
        base.clone()
            .fingerprint(project, NativeProtocol::CodexExec, config)
    );

    let other_project = ProjectId::from_uuid(Uuid::from_u128(2)).unwrap();
    let mut other_id = base.clone();
    other_id.request_id = Uuid::from_u128(0x5678);
    let other_harness = request(AgentKind::Claude, PROMPT);
    let other_prompt = request(AgentKind::Codex, "a different prompt");
    let variants = [
        base.fingerprint(other_project, NativeProtocol::CodexExec, config),
        other_id.fingerprint(project, NativeProtocol::CodexExec, config),
        other_harness.fingerprint(project, NativeProtocol::CodexExec, config),
        other_prompt.fingerprint(project, NativeProtocol::CodexExec, config),
        base.fingerprint(project, NativeProtocol::CodexAppServer, config),
        base.fingerprint(
            project,
            NativeProtocol::CodexExec,
            Sha256Digest::of(b"other"),
        ),
    ];
    for (index, variant) in variants.iter().enumerate() {
        assert_ne!(*variant, fingerprint, "variant {index}");
        for other in &variants[index + 1..] {
            assert_ne!(variant, other);
        }
    }
}

#[test]
fn launch_argv_is_owned_by_the_protocol_profile() {
    let argv = |route: &DispatchRoute, mode| build_launch_spec(route, mode).unwrap().arguments;
    let codex_exec = route(AgentKind::Codex, NativeProtocol::CodexExec, Some("gpt-5"));
    assert_eq!(
        argv(&codex_exec, SessionMode::Execute),
        [
            "exec",
            "--json",
            "--sandbox",
            "read-only",
            "--ephemeral",
            "--model",
            "gpt-5",
            "-"
        ]
    );
    assert_eq!(
        build_launch_spec(&codex_exec, SessionMode::Probe).unwrap_err(),
        DispatchError::ProbeUnsupported
    );
    let app_server = route(
        AgentKind::Codex,
        NativeProtocol::CodexAppServer,
        Some("gpt-5"),
    );
    for mode in [SessionMode::Probe, SessionMode::Execute] {
        assert_eq!(argv(&app_server, mode), ["app-server", "--stdio"]);
    }
    let claude = route(AgentKind::Claude, NativeProtocol::ClaudeStreamJson, None);
    let claude_argv = [
        "--bare",
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--permission-mode",
        "dontAsk",
        "--tools=",
        "--strict-mcp-config",
        "--restricted",
        "--no-session-persistence",
    ];
    assert_eq!(argv(&claude, SessionMode::Execute), claude_argv);
    let claude_model = route(
        AgentKind::Claude,
        NativeProtocol::ClaudeStreamJson,
        Some("claude-opus-5-5"),
    );
    let mut with_model: Vec<&str> = claude_argv.to_vec();
    with_model.extend(["--model", "claude-opus-5-5"]);
    assert_eq!(argv(&claude_model, SessionMode::Probe), with_model);
    for (harness, expected) in [
        (AgentKind::OpenCode, vec!["acp"]),
        (AgentKind::Copilot, vec!["--acp", "--no-auto-update"]),
        (AgentKind::Grok, vec!["agent", "stdio"]),
    ] {
        let acp = route(harness, NativeProtocol::Acp, None);
        assert_eq!(argv(&acp, SessionMode::Execute), expected, "{harness:?}");
    }
    assert_eq!(
        build_launch_spec(&codex_exec, SessionMode::Execute)
            .unwrap()
            .executable,
        PathBuf::from(native_executable(&format!("{PRIVATE_SENTINEL}_codex")))
    );
}

#[test]
fn launch_environment_lists_system_names_first_without_duplicates() {
    let mut value = route_json("codex", "codex_exec");
    value["environment_names"] = json!(["OPENAI_API_KEY", "TEMP", "PATH"]);
    let route = parse_route(value).unwrap().routes.remove(0);
    let spec = build_launch_spec(&route, SessionMode::Execute).unwrap();
    let mut expected: Vec<&str> = SYSTEM_ENVIRONMENT_NAMES.to_vec();
    expected.extend(["OPENAI_API_KEY", "PATH"]);
    assert_eq!(spec.environment_names, expected);
}
