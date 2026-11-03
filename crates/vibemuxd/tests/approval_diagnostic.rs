#![cfg(all(windows, feature = "test_helpers"))]
//! Native process diagnostics use the exact built synthetic image by default.
use std::{path::PathBuf, time::Duration};
use tokio::sync::watch;
use vibemuxd::chat_session::approval_diagnostic::{
    DiagnosticCandidate, DiagnosticLimits, DiagnosticPreparationError, DiagnosticStop,
};

fn limits() -> DiagnosticLimits {
    DiagnosticLimits {
        initialize: Duration::from_secs(5),
        turn: Duration::from_secs(5),
        shutdown: Duration::from_secs(2),
        ..DiagnosticLimits::default()
    }
}

async fn fixture() -> (tempfile::TempDir, DiagnosticCandidate) {
    let temp = tempfile::tempdir().expect("owned fixture namespace");
    let vendor = temp.path().join("vendor_空间");
    std::fs::create_dir(&vendor).expect("owned vendor folder");
    let candidate = DiagnosticCandidate::prepare_fixture(temp.path().to_owned(), vendor, limits())
        .await
        .expect("guarded fixture candidate");
    (temp, candidate)
}

#[tokio::test]
async fn a_contained_fixture_is_denied_without_fabricating_current_input_proof() {
    let (_temp, candidate) = fixture().await;
    let summary = candidate.summary();
    assert!(summary.fixture_only);
    assert!(
        !summary.affirmative_response_available
            && !summary.policy_sources_qualified
            && !summary.production_profile_qualified
    );
    let debug = format!("{candidate:?}");
    assert!(
        !debug.contains(
            candidate
                .workspace_path()
                .to_str()
                .expect("UTF-8 test path")
        )
    );
    let (cancel, _) = watch::channel(false);
    let result = candidate
        .run(cancel.subscribe())
        .await
        .expect("owned fixture diagnostic");
    assert_eq!(result.report.stop, DiagnosticStop::ObservedTerminal);
    assert!(
        result.report.initialize_written
            && result.report.input_written
            && result.report.denial_written
    );
    assert!(result.report.observations.matching_request_seen && result.report.observations.denied);
    assert!(!result.report.observations.proof_before_reply);
    assert_eq!(result.probe_artifact_present, Some(false));
    assert!(
        result.report.cleanup.root_reaped
            && result.report.cleanup.tree_empty
            && result.report.cleanup.cleanup_verified
    );
    assert!(!result.workspace_retained);
    let report = serde_json::to_string(&result).expect("content-free report");
    assert!(
        !report.contains("approved fixture")
            && !report.contains("file_path")
            && !report.contains("tool_use_id")
    );
}

#[tokio::test]
async fn a_closed_cancellation_owner_prevents_even_fixture_startup() {
    let (temp, candidate) = fixture().await;
    let (sender, receiver) = watch::channel(false);
    drop(sender);
    assert_eq!(
        candidate
            .run(receiver)
            .await
            .expect_err("closed owner refuses before acquisition"),
        DiagnosticPreparationError::Cancelled
    );
    assert!(!temp.path().join("vendor_空间/diagnostic_started").exists());
}

#[tokio::test]
async fn changed_workspace_or_fixture_image_is_refused_before_launch() {
    for replace_image in [false, true] {
        let (temp, candidate) = fixture().await;
        let workspace = candidate.workspace_path().to_owned();
        if replace_image {
            std::fs::write(
                temp.path().join("vendor_空间/claude.exe"),
                b"changed fixture image",
            )
            .expect("owned copy change");
        } else {
            std::fs::write(workspace.join("retain.txt"), b"unrecognized file")
                .expect("unknown workspace file");
        }
        let (cancel_owner, cancel) = watch::channel(false);
        let error = candidate
            .run(cancel)
            .await
            .expect_err("changed admission must refuse");
        drop(cancel_owner);
        assert!(matches!(
            error,
            DiagnosticPreparationError::SourceChanged
                | DiagnosticPreparationError::WorkspaceChanged
        ));
        assert!(!workspace.join("approval_diagnostic.txt").exists());
        if !replace_image {
            assert_eq!(
                std::fs::read(workspace.join("retain.txt")).expect("file retained"),
                b"unrecognized file"
            );
        }
    }
}

#[tokio::test]
async fn native_preparation_is_fixed_and_does_not_claim_version_or_policy_qualification() {
    let temp = tempfile::tempdir().expect("owned candidate namespace");
    let image = temp.path().join("claude.exe");
    std::fs::write(&image, b"this is never executed").expect("non-executable source");
    let candidate =
        DiagnosticCandidate::prepare_native(image, temp.path().to_owned(), Vec::new(), limits())
            .await
            .expect("preparation only");
    assert!(!candidate.summary().fixture_only && !candidate.summary().policy_sources_qualified);
    let spec = candidate.launch_spec();
    for flag in [
        "--restricted",
        "--safe-mode",
        "--strict-mcp-config",
        "--no-session-persistence",
        "--tools=Write",
        "--no-chrome",
    ] {
        assert!(spec.arguments.iter().any(|value| value == flag));
    }
    assert!(
        spec.arguments
            .windows(2)
            .any(|pair| pair == ["--permission-mode", "manual"])
    );
    assert!(
        spec.arguments
            .windows(2)
            .any(|pair| pair == ["--permission-prompts", "host"])
    );
    assert!(
        spec.arguments
            .windows(2)
            .any(|pair| pair == ["--max-turns", "1"])
    );
    assert!(!spec.arguments.iter().any(|value| value.contains("bypass")
        || value == "auto"
        || value == "--allowedTools"
        || value == "--resume"));
    let workspace = candidate.workspace_path().to_owned();
    candidate
        .discard()
        .await
        .expect("remove only still-empty owned directory");
    assert!(!workspace.exists());
}

#[tokio::test]
async fn an_environment_override_cannot_select_an_unreviewed_cli_capability() {
    let result = DiagnosticCandidate::prepare_native(
        PathBuf::from("never.exe"),
        PathBuf::from("never"),
        vec!["CLAUDE_CODE_ENABLE_UNREVIEWED_CAPABILITY".into()],
        limits(),
    )
    .await;
    assert_eq!(
        result.expect_err("unreviewed environment"),
        DiagnosticPreparationError::InvalidConfiguration
    );
}

#[tokio::test]
#[ignore = "Requires an explicit installed-vendor diagnostic allowance and surviving-policy review; can consume model resources"]
async fn installed_claude_deny_only_diagnostic() {
    let executable = std::env::var_os("VIBEMUX_APPROVAL_PROBE_EXECUTABLE")
        .map(PathBuf::from)
        .expect("provide the reviewed exact native Claude 2.1.304 executable");
    let names = std::env::var("VIBEMUX_APPROVAL_PROBE_ENVIRONMENT_NAMES")
        .unwrap_or_default()
        .split(',')
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect();
    let root = std::env::var_os("VIBEMUX_APPROVAL_PROBE_ROOT")
        .map(PathBuf::from)
        .expect("provide a new exact diagnostic root owned by this invocation");
    assert!(root.is_absolute(), "explicit absolute diagnostic root");
    std::fs::create_dir(&root)
        .expect("create_new diagnostic namespace; never reuse unknown resources");
    let candidate = DiagnosticCandidate::prepare_native(
        executable,
        root.clone(),
        names,
        DiagnosticLimits::default(),
    )
    .await
    .expect("reviewed candidate prepared");
    let (cancel_owner, cancel) = watch::channel(false);
    let report = candidate
        .run(cancel)
        .await
        .expect("explicit native diagnostic completed");
    drop(cancel_owner);
    println!(
        "{}",
        serde_json::to_string(&report).expect("content-free observations")
    );
    // Native proof may be absent: observation is the purpose, not a green certificate.
    assert!(
        report.report.cleanup.cleanup_verified,
        "retain ownership when cleanup cannot be confirmed"
    );
    assert!(!report.candidate.production_profile_qualified);
    // A failed run or unknown files retain the explicit recovery namespace.
    if !report.workspace_retained {
        std::fs::remove_dir(root).expect("remove only empty known namespace");
    }
}
