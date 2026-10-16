#![cfg(feature = "test_helpers")]
//! DSH session service contract (ADR 032 §§4–10) over the real control IPC,
//! against the synthetic SDK runtime.
//!
//! The property under test is the whole Stage 3 path: control request →
//! service → contained child process → framed records → cursor pages. It
//! proves the launcher-pair trust rules, the handshake gate, one-turn-at-a-time
//! admission, structured terminal evidence, cancellation by tree termination,
//! teardown, and the rule that no prompt text reaches a status view.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use vibemux_harness::dsh::{DshPhase, DshRecordKind, DshTurnOutcome};
use vibemuxd::control::{
    ControlClient, ControlError, DaemonControlServer, DshSessionOpenRequest, DshSessionPollQuery,
    DshSessionPromptRequest, DshSessionRecord, DshSessionStatus,
};

const FIXTURE: &str = env!("CARGO_BIN_EXE_vibemux_dsh_service_fixture");
/// The SDK session id the tests use; the protocol lets the client choose it.
const SESSION_ID: &str = "vibemux_test_session";
/// Turns the page-snapshot test streams; each is one `flood` burst.
const FLOOD_TURNS: u64 = 20;

fn remote(code: &str) -> ControlError {
    ControlError::Remote {
        code: code.to_string(),
    }
}

/// Scratch space under the workspace `target/` tree: the platform temporary
/// directory is not writable in every sandbox this project runs in, and
/// `target/` is already ignored by Git.
fn scratch(label: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch");
    std::fs::create_dir_all(&root).expect("scratch root");
    let directory = root.join(format!("dsh-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("scratch");
    directory
}

/// One project layout: a project root with a `.vibemux` state dir, and a
/// launcher pair outside it, exactly as the trust rules require.
struct Layout {
    directory: PathBuf,
    state_dir: PathBuf,
}

impl Layout {
    fn new(label: &str) -> Self {
        let directory = scratch(label);
        let state_dir = directory.join("project").join(".vibemux");
        std::fs::create_dir_all(&state_dir).expect("state dir");
        Self {
            directory,
            state_dir,
        }
    }

    /// Installs the synthetic runtime as `node.exe`, and a minimal
    /// `@deepseek-ai/dsh` package whose `bin` map names the entry script.
    /// Both live outside the project root.
    fn install_launcher_pair(&self) -> (PathBuf, PathBuf) {
        let node = self
            .directory
            .join(format!("node{}", std::env::consts::EXE_SUFFIX));
        std::fs::copy(FIXTURE, &node).expect("install fixture as node");

        let package = self.directory.join("node_modules/@deepseek-ai/dsh");
        std::fs::create_dir_all(package.join("lib")).expect("package dirs");
        std::fs::write(
            package.join("package.json"),
            json!({"name": "@deepseek-ai/dsh", "bin": {"dsh": "lib/bin.js"}}).to_string(),
        )
        .expect("manifest");
        let script = package.join("lib/bin.js");
        // The fixture never executes this file; it is the trust anchor, and
        // the fixture treats an unrecognized first argument as `serve`.
        std::fs::write(&script, "// synthetic entry point\n").expect("entry script");
        (node, script)
    }

    fn write_config(&self, executable: &Path, entry_script: &Path, extra: Value) {
        let mut config = json!({
            "schema_version": 1,
            "executable": executable,
            "entry_script": entry_script,
            "provider": "deepseek-official",
            "model": "deepseek-v4-flash",
            "limits": {
                "initialize_deadline_ms": 5_000,
                "turn_deadline_ms": 5_000,
                "shutdown_grace_ms": 1_000,
            },
        });
        if let Some(object) = extra.as_object() {
            for (key, value) in object {
                config[key] = value.clone();
            }
        }
        std::fs::write(
            self.state_dir.join("dsh_session.json"),
            serde_json::to_vec_pretty(&config).expect("encode config"),
        )
        .expect("config");
    }

    fn database_path(&self) -> PathBuf {
        self.state_dir.join("state.sqlite3")
    }

    fn runtime_dir(&self) -> PathBuf {
        self.directory.join("runtime")
    }

    async fn server(&self) -> DaemonControlServer {
        DaemonControlServer::start(&self.database_path(), &self.runtime_dir())
            .await
            .expect("control server")
    }
}

/// Starts a configured server and returns the client plus its layout.
async fn configured_server(label: &str) -> (DaemonControlServer, ControlClient, Layout) {
    let layout = Layout::new(label);
    let (node, script) = layout.install_launcher_pair();
    layout.write_config(&node, &script, json!({}));
    let server = layout.server().await;
    let client = ControlClient::from_descriptor(server.descriptor_path()).expect("client");
    (server, client, layout)
}

/// Every record after a starting cursor, and the status once the turn settled.
struct SettledTurn {
    records: Vec<DshSessionRecord>,
    status: DshSessionStatus,
}

fn settled(status: &DshSessionStatus) -> bool {
    let finished = status.completed_turns
        + status.failed_turns
        + status.cancelled_turns
        + status.unverified_turns;
    finished > 0 && status.phase == DshPhase::Ready
}

/// Polls until the turn settles or the deadline expires, collecting every
/// record after `cursor`. Each poll advances the cursor, so a record an
/// early poll returned never reappears on a later page. A page reads its
/// status after its records, so a settled status can arrive before the
/// last records do. The helper therefore polls again after the first settled
/// page and stops at the first later page that has nothing more.
async fn poll_until_settled(client: &ControlClient, cursor: u64) -> SettledTurn {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let mut cursor = cursor;
    let mut records = Vec::new();
    let mut seen_settled = false;
    let mut last_status = None;
    while std::time::Instant::now() < deadline {
        let page = client
            .dsh_session_poll(DshSessionPollQuery { cursor })
            .await
            .expect("poll");
        assert!(!page.truncated, "the retained window dropped records");
        cursor = page.next;
        records.extend(page.records);
        if seen_settled && !page.more {
            return SettledTurn {
                records,
                status: page.status,
            };
        }
        seen_settled |= settled(&page.status);
        if !page.more {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        last_status = Some(page.status);
    }
    panic!(
        "the turn never settled: {last_status:?} after {} records",
        records.len()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_turn_runs_through_the_control_surface() {
    let (server, client, _layout) = configured_server("full-turn").await;

    let opened = client
        .dsh_session_open(&DshSessionOpenRequest {
            session_id: Some(SESSION_ID.to_string()),
        })
        .await
        .expect("open");
    assert_eq!(opened.session_id, SESSION_ID);
    assert_eq!(opened.phase, DshPhase::Ready);
    assert_eq!(opened.provider, "deepseek-official");
    assert_eq!(opened.server_version.as_deref(), Some("0.0.1"));
    assert_eq!(opened.config.profile, "sdk");
    // The status view carries no filesystem path.
    let encoded = serde_json::to_string(&opened).expect("encode status");
    for hidden in ["node.exe", "bin.js", "entry_script", "executable"] {
        assert!(!encoded.contains(hidden), "{hidden}");
    }

    let prompt = "a prompt that must never appear in a status view";
    let prompted = client
        .dsh_session_prompt(&DshSessionPromptRequest {
            prompt: prompt.to_string(),
        })
        .await
        .expect("prompt");
    assert_eq!(prompted.prompt_bytes_total, prompt.len() as u64);
    assert!(
        !serde_json::to_string(&prompted)
            .expect("encode")
            .contains("must never appear")
    );

    let turn = poll_until_settled(&client, 0).await;
    assert_eq!(turn.status.completed_turns, 1);
    assert_eq!(turn.status.last_outcome, Some(DshTurnOutcome::Completed));
    // Sequences are strictly increasing: no record was read twice.
    assert!(
        turn.records
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
    assert!(
        turn.records
            .iter()
            .any(|record| record.kind == DshRecordKind::Handshake)
    );
    assert!(
        turn.records
            .iter()
            .any(|record| record.kind == DshRecordKind::TurnStart)
    );
    assert!(
        turn.records
            .iter()
            .any(|record| record.kind == DshRecordKind::TurnEnd)
    );
    // Idle status is captured as a diagnostic, never as the outcome.
    assert!(
        turn.records
            .iter()
            .any(|record| record.kind == DshRecordKind::Status)
    );
    // Raw runtime content is byte-exact and only reachable through the poll.
    assert!(
        turn.records
            .iter()
            .any(|record| record.raw_json.contains("fixture reply"))
    );

    let closed = client.dsh_session_close().await.expect("close");
    assert_eq!(closed.phase, DshPhase::Closed);
    // The session is gone; a second poll reports the actionable cause.
    assert_eq!(
        client
            .dsh_session_poll(DshSessionPollQuery::default())
            .await,
        Err(remote("dsh_session_not_found"))
    );

    server.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_open_is_refused_while_one_session_is_live() {
    let (server, client, _layout) = configured_server("busy").await;
    client
        .dsh_session_open(&DshSessionOpenRequest {
            session_id: Some(SESSION_ID.to_string()),
        })
        .await
        .expect("first open");
    assert_eq!(
        client
            .dsh_session_open(&DshSessionOpenRequest::default())
            .await,
        Err(remote("dsh_session_busy"))
    );
    // The live session is untouched by the refusal.
    let status = client.dsh_session_status().await.expect("status");
    assert_eq!(status.phase, DshPhase::Ready);
    client.dsh_session_close().await.expect("close");
    server.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_terminates_the_runtime_and_records_the_turn() {
    // The fixture selects its behaviour from `--profile`, so the config's
    // `profile` field is what makes it stream a turn that never ends.
    let layout = Layout::new("cancel");
    let (node, script) = layout.install_launcher_pair();
    layout.write_config(&node, &script, json!({"profile": "no-turn-end"}));
    let server = layout.server().await;
    let client = ControlClient::from_descriptor(server.descriptor_path()).expect("client");
    client
        .dsh_session_open(&DshSessionOpenRequest {
            session_id: Some(SESSION_ID.to_string()),
        })
        .await
        .expect("open");
    client
        .dsh_session_prompt(&DshSessionPromptRequest {
            prompt: "cancel me".to_string(),
        })
        .await
        .expect("prompt");
    let cancelled = client.dsh_session_cancel().await.expect("cancel");
    assert_eq!(cancelled.cancelled_turns, 1);
    assert_eq!(cancelled.last_outcome, Some(DshTurnOutcome::Cancelled));
    // Cancellation terminates the contained tree, so the transport closes.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let status = client.dsh_session_status().await.expect("status");
        if !status.transport_open {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the runtime outlived its cancellation"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    client.dsh_session_close().await.expect("close");
    server.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_untrusted_launcher_pair_fails_closed_before_any_process_starts() {
    let layout = Layout::new("untrusted");
    let (node, script) = layout.install_launcher_pair();
    // A `.ps1` route is refused for every protocol: it would route argv
    // through a command interpreter.
    let shim = layout.directory.join("dsh.ps1");
    std::fs::write(&shim, "# shim").expect("shim");
    layout.write_config(&shim, &script, json!({}));
    let server = layout.server().await;
    let client = ControlClient::from_descriptor(server.descriptor_path()).expect("client");
    assert_eq!(
        client
            .dsh_session_open(&DshSessionOpenRequest::default())
            .await,
        Err(remote("dsh_session_config_invalid"))
    );

    // A script the DSH package does not name is refused too.
    let stray = layout.directory.join("stray.js");
    std::fs::write(&stray, "// stray").expect("stray");
    layout.write_config(&node, &stray, json!({}));
    assert_eq!(
        client
            .dsh_session_open(&DshSessionOpenRequest::default())
            .await,
        Err(remote("dsh_session_config_invalid"))
    );
    server.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_field_in_the_configuration_is_refused() {
    let layout = Layout::new("unknown-field");
    let (node, script) = layout.install_launcher_pair();
    layout.write_config(&node, &script, json!({"argv": ["--yolo"]}));
    let server = layout.server().await;
    let client = ControlClient::from_descriptor(server.descriptor_path()).expect("client");
    assert_eq!(
        client
            .dsh_session_open(&DshSessionOpenRequest::default())
            .await,
        Err(remote("dsh_session_config_invalid"))
    );
    server.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_handshake_that_never_answers_reports_the_driver_timeout() {
    let layout = Layout::new("hang");
    let (node, script) = layout.install_launcher_pair();
    layout.write_config(
        &node,
        &script,
        json!({
            "profile": "hang",
            "limits": {
                "initialize_deadline_ms": 1_000,
                "turn_deadline_ms": 1_000,
                "shutdown_grace_ms": 1_000,
            },
        }),
    );
    let server = layout.server().await;
    let client = ControlClient::from_descriptor(server.descriptor_path()).expect("client");
    assert_eq!(
        client
            .dsh_session_open(&DshSessionOpenRequest::default())
            .await,
        Err(remote("dsh_session_refused"))
    );
    // The failed attempt left no live session behind.
    assert_eq!(
        client.dsh_session_status().await,
        Err(remote("dsh_session_not_found"))
    );
    server.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_prompt_is_refused_before_a_session_exists() {
    let (server, client, _layout) = configured_server("no-session").await;
    assert_eq!(
        client.dsh_session_status().await,
        Err(remote("dsh_session_not_found"))
    );
    assert_eq!(
        client
            .dsh_session_prompt(&DshSessionPromptRequest {
                prompt: "too early".to_string(),
            })
            .await,
        Err(remote("dsh_session_not_found"))
    );
    server.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_invalid_prompt_is_refused_by_the_service() {
    let (server, client, _layout) = configured_server("empty-prompt").await;
    client
        .dsh_session_open(&DshSessionOpenRequest::default())
        .await
        .expect("open");
    assert_eq!(
        client
            .dsh_session_prompt(&DshSessionPromptRequest {
                prompt: "   \n".to_string(),
            })
            .await,
        Err(remote("dsh_session_prompt_invalid"))
    );
    let status = client.dsh_session_status().await.expect("status");
    assert_eq!(status.turns_started, 0);
    client.dsh_session_close().await.expect("close");
    server.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_daemon_shuts_a_live_session_down_with_itself() {
    let (server, client, _layout) = configured_server("daemon-shutdown").await;
    client
        .dsh_session_open(&DshSessionOpenRequest::default())
        .await
        .expect("open");
    let status = client.dsh_session_status().await.expect("status");
    assert!(status.transport_open);
    assert_eq!(status.phase, DshPhase::Ready);
    // Shutting the daemon down must join the session, so the vendor process
    // cannot outlive it. A leaked process would keep this test binary's
    // stdout pipe open and the fixture would leak past the run.
    server.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_poll_page_status_never_runs_ahead_of_its_records() {
    // The `flood` profile streams hundreds of records per turn, so the
    // runtime reader appends records while the polls below are served. A page
    // whose status were read after its records could report a head, or a
    // settled turn, that those records do not reach yet.
    let layout = Layout::new("page-snapshot");
    let (node, script) = layout.install_launcher_pair();
    layout.write_config(&node, &script, json!({"profile": "flood"}));
    let server = layout.server().await;
    let client = ControlClient::from_descriptor(server.descriptor_path()).expect("client");
    client
        .dsh_session_open(&DshSessionOpenRequest {
            session_id: Some(SESSION_ID.to_string()),
        })
        .await
        .expect("open");

    let mut cursor = 0;
    let mut turn_ends_read = 0_u64;
    let mut drained_pages = 0_u64;
    for turn in 1..=FLOOD_TURNS {
        client
            .dsh_session_prompt(&DshSessionPromptRequest {
                prompt: format!("flood turn {turn}"),
            })
            .await
            .expect("prompt");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "turn {turn} never drained"
            );
            // No sleep: the next poll should land while the burst is running.
            let page = client
                .dsh_session_poll(DshSessionPollQuery { cursor })
                .await
                .expect("poll");
            assert!(!page.truncated, "the retained window dropped records");
            turn_ends_read += page
                .records
                .iter()
                .filter(|record| record.kind == DshRecordKind::TurnEnd)
                .count() as u64;
            cursor = page.next;
            if page.more {
                continue;
            }
            drained_pages += 1;
            // With nothing more to read, the client holds every record up to
            // the head the page's status reports...
            assert_eq!(
                page.next, page.status.records,
                "turn {turn}: the page status counts records its page does not reach"
            );
            // ...so every turn the status counts as completed has its
            // `TurnEnd` record in the client's hands.
            assert_eq!(
                page.status.completed_turns, turn_ends_read,
                "turn {turn}: the page status settled a turn whose TurnEnd was not returned"
            );
            if page.status.completed_turns == turn && page.status.phase == DshPhase::Ready {
                break;
            }
        }
    }
    // Every drained page was checked, including those served mid-burst.
    assert!(drained_pages >= FLOOD_TURNS);

    client.dsh_session_close().await.expect("close");
    server.shutdown().await.expect("shutdown");
}
