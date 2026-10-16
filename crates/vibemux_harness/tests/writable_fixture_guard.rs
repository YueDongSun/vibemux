#![cfg(feature = "test_helpers")]

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::json;
use uuid::Uuid;
use vibemux_harness::{
    AgentKind,
    dispatch::{
        DispatchError, DispatchRoute, FixtureLaunchGuard, NativeProtocol,
        build_writable_fixture_launch_spec, writable_profile::WritableGrant,
    },
};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("vibemux_fixture_guard_{}", Uuid::new_v4().simple()));
        fs::create_dir(&path).expect("unique test root");
        Self(fs::canonicalize(path).expect("canonical test root"))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn fixture_path(root: &Path, alias: AgentKind) -> PathBuf {
    root.join(format!(
        "{}{}",
        alias.command_name(),
        std::env::consts::EXE_SUFFIX
    ))
}

fn fixture_route(alias: AgentKind, executable: &Path) -> DispatchRoute {
    let protocol = match alias {
        AgentKind::Codex => NativeProtocol::CodexExec,
        AgentKind::Claude => NativeProtocol::ClaudeStreamJson,
        AgentKind::OpenCode => NativeProtocol::Acp,
        _ => panic!("test alias must be Codex or Claude"),
    };
    serde_json::from_value(json!({
        "harness": alias,
        "protocol": protocol,
        "executable": executable,
        "enabled": true,
        "allow_execution": true,
        "environment_names": ["FIXTURE_MARKER"],
    }))
    .expect("test route")
}

fn prepare_copies() -> (Scratch, PathBuf, PathBuf, FixtureLaunchGuard) {
    let scratch = Scratch::new();
    let source = scratch
        .path()
        .join(format!("built_worker{}", std::env::consts::EXE_SUFFIX));
    fs::write(&source, b"synthetic built worker image v1").expect("built source");
    let vendor = scratch.path().join("vendor");
    fs::create_dir(&vendor).expect("vendor root");
    for alias in [AgentKind::Codex, AgentKind::Claude] {
        fs::copy(&source, fixture_path(&vendor, alias)).expect("owned worker copy");
    }
    let guard = FixtureLaunchGuard::register_worker_copies(
        &source,
        &vendor,
        &[AgentKind::Codex, AgentKind::Claude],
    )
    .expect("register exact copies");
    (scratch, source, vendor, guard)
}

#[test]
fn registered_copies_get_distinct_profiles_and_strict_grant_json() {
    let (_scratch, _source, vendor, guard) = prepare_copies();
    let grant = WritableGrant {
        read_files: true,
        search_files: false,
        edit_files: false,
        write_files: true,
        run_dev_tests: false,
    };
    for (alias, expected_profile) in [
        (AgentKind::Codex, "codex_fixture_writable_v1"),
        (AgentKind::Claude, "claude_fixture_writable_v1"),
    ] {
        let route = fixture_route(alias, &fixture_path(&vendor, alias));
        let spec = build_writable_fixture_launch_spec(&route, grant, &guard).expect("fixture spec");
        let profile = spec
            .arguments
            .iter()
            .position(|arg| arg == "--fixture_profile")
            .expect("profile arg");
        assert_eq!(spec.arguments[profile + 1], expected_profile);
        let json_arg = spec
            .arguments
            .iter()
            .position(|arg| arg == "--fixture_grant_json")
            .expect("grant arg");
        let parsed: WritableGrant =
            serde_json::from_str(&spec.arguments[json_arg + 1]).expect("strict grant JSON");
        assert_eq!(parsed, grant);
        assert_eq!(
            spec.environment_names.last().map(String::as_str),
            Some("FIXTURE_MARKER")
        );
        assert!(!format!("{guard:?}").contains("vendor"));
        if alias == AgentKind::Codex {
            assert_eq!(spec.arguments.last().map(String::as_str), Some("-"));
        }
    }
}

#[test]
fn byte_or_path_change_is_refused_at_each_launch() {
    let (scratch, source, vendor, guard) = prepare_copies();
    let path = fixture_path(&vendor, AgentKind::Codex);
    let route = fixture_route(AgentKind::Codex, &path);
    fs::write(&path, b"changed worker copy").expect("mutate owned copy");
    assert_eq!(
        build_writable_fixture_launch_spec(&route, WritableGrant::NONE, &guard),
        Err(DispatchError::ConfigExecutableInvalid)
    );
    fs::copy(&source, &path).expect("restore owned copy");
    build_writable_fixture_launch_spec(&route, WritableGrant::NONE, &guard)
        .expect("restored exact worker copy");
    let outside = scratch
        .path()
        .join(format!("codex{}", std::env::consts::EXE_SUFFIX));
    fs::write(&outside, b"synthetic built worker image v1").expect("outside image");
    let outside_route = fixture_route(AgentKind::Codex, &outside);
    assert_eq!(
        build_writable_fixture_launch_spec(&outside_route, WritableGrant::NONE, &guard),
        Err(DispatchError::ConfigExecutableInvalid)
    );
    fs::write(&source, b"changed source image").expect("mutate owned source");
    assert_eq!(
        build_writable_fixture_launch_spec(&route, WritableGrant::NONE, &guard),
        Err(DispatchError::ConfigExecutableInvalid)
    );
}

#[test]
fn a_route_cannot_register_another_alias_or_a_source_inside_vendor() {
    let (_scratch, source, vendor, guard) = prepare_copies();
    let unregistered = fixture_route(
        AgentKind::OpenCode,
        &fixture_path(&vendor, AgentKind::Codex),
    );
    assert_eq!(
        build_writable_fixture_launch_spec(&unregistered, WritableGrant::NONE, &guard),
        Err(DispatchError::ExecutionDisabled)
    );
    assert!(
        FixtureLaunchGuard::register_worker_copies(
            &source,
            &vendor,
            &[AgentKind::Codex, AgentKind::Codex]
        )
        .is_err()
    );
    assert!(
        FixtureLaunchGuard::register_worker_copies(
            &fixture_path(&vendor, AgentKind::Codex),
            &vendor,
            &[AgentKind::Claude]
        )
        .is_err()
    );
}

#[cfg(unix)]
#[test]
fn symlinked_worker_copy_is_rejected() {
    use std::os::unix::fs::symlink;
    let (_scratch, source, vendor, _guard) = prepare_copies();
    let path = fixture_path(&vendor, AgentKind::Codex);
    fs::remove_file(&path).expect("owned copy");
    symlink(&source, &path).expect("owned symlink");
    assert!(
        FixtureLaunchGuard::register_worker_copies(&source, &vendor, &[AgentKind::Codex]).is_err()
    );
}

#[cfg(windows)]
#[test]
fn symlinked_worker_copy_is_rejected_when_supported() {
    use std::os::windows::fs::symlink_file;
    let (_scratch, source, vendor, _guard) = prepare_copies();
    let path = fixture_path(&vendor, AgentKind::Codex);
    fs::remove_file(&path).expect("owned copy");
    if let Err(error) = symlink_file(&source, &path) {
        eprintln!("skipped: creating test-owned symlink requires host privilege ({error})");
        return;
    }
    assert!(
        FixtureLaunchGuard::register_worker_copies(&source, &vendor, &[AgentKind::Codex]).is_err()
    );
}
