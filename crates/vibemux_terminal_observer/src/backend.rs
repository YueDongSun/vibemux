//! Read-only WezTerm inventory and explicit focus. Terminal text stays native.

use crate::config::ObserverConfig;
use serde::Deserialize;
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command, time::timeout};
use vibemux_plugin_protocol::{
    terminal::{MAX_CWD_BYTES, MAX_TERMINAL_PANES, valid_inventory, valid_label},
    wire::{TerminalFocusRequest, TerminalInventory, TerminalPane},
};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_OUTPUT_BYTES: usize = 512 * 1024;
const MAX_CLEANUP_SOURCE_PANES: usize = 256;

pub async fn inventory(
    config: &ObserverConfig,
    expected_cwd: &str,
) -> Result<TerminalInventory, &'static str> {
    if !valid_label(expected_cwd, MAX_CWD_BYTES) || !Path::new(expected_cwd).is_absolute() {
        return Err("terminal_invalid_cwd");
    }
    let instance_id = config.verify_instance()?;
    let output = command(config, &["list", "--format", "json"]).await?;
    let panes = parse_inventory(&output, expected_cwd)?;
    if config.verify_instance()? != instance_id {
        return Err("terminal_instance_changed");
    }
    let inventory = TerminalInventory { instance_id, panes };
    if !valid_inventory(&inventory) {
        return Err("terminal_invalid_inventory");
    }
    Ok(inventory)
}

/// Complete-or-error subtree observation for the opt-in cleanup capability.
pub async fn cleanup_inventory(
    config: &ObserverConfig,
    expected_root: &str,
    worker: &crate::observation_worker::ObservationWorker,
) -> Result<TerminalInventory, &'static str> {
    if !config.cleanup_inventory_enabled {
        return Err("terminal_permission_denied");
    }
    if !valid_label(expected_root, MAX_CWD_BYTES) || !cleanup_local_path(Path::new(expected_root)) {
        return Err("terminal_invalid_cwd");
    }
    let runtime =
        tokio::runtime::Handle::try_current().map_err(|_| "terminal_cleanup_incomplete")?;
    let expected_root = expected_root.to_owned();
    let config = config.clone();
    worker
        .run(move || {
            let instance_id = config.verify_instance()?;
            let output = runtime.block_on(command(&config, &["list", "--format", "json"]))?;
            let panes = parse_cleanup_inventory(&output, &expected_root)?;
            if config.verify_instance()? != instance_id {
                return Err("terminal_instance_changed");
            }
            let inventory = TerminalInventory { instance_id, panes };
            if !valid_inventory(&inventory) {
                return Err("terminal_invalid_inventory");
            }
            Ok(inventory)
        })
        .await
}

pub async fn focus(
    config: &ObserverConfig,
    request: &TerminalFocusRequest,
) -> Result<(), &'static str> {
    let current = inventory(config, &request.expected_cwd).await?;
    if current.instance_id != request.instance_id
        || !current
            .panes
            .iter()
            .any(|pane| pane.pane_id == request.pane_id)
    {
        return Err("terminal_identity_mismatch");
    }
    command(config, &["activate-pane", "--pane-id", &request.pane_id]).await?;
    if config.verify_instance()? != request.instance_id {
        return Err("terminal_instance_changed");
    }
    Ok(())
}

async fn command(config: &ObserverConfig, arguments: &[&str]) -> Result<Vec<u8>, &'static str> {
    let mut command = Command::new(&config.wezterm_executable);
    command
        .args(["--skip-config", "cli", "--no-auto-start"])
        .args(arguments)
        .env_clear()
        .env("WEZTERM_UNIX_SOCKET", &config.socket_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    // Windows loader support only; no provider or shell environment inheritance.
    for key in ["SYSTEMROOT", "WINDIR", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command.spawn().map_err(|_| "terminal_command_failed")?;
    let stdout = child.stdout.take().ok_or("terminal_command_failed")?;
    let result = timeout(COMMAND_TIMEOUT, async {
        let mut output = Vec::new();
        stdout
            .take((MAX_OUTPUT_BYTES + 1) as u64)
            .read_to_end(&mut output)
            .await
            .map_err(|_| "terminal_command_failed")?;
        if output.len() > MAX_OUTPUT_BYTES {
            return Err("terminal_output_too_large");
        }
        let status = child.wait().await.map_err(|_| "terminal_command_failed")?;
        if !status.success() {
            return Err("terminal_command_failed");
        }
        Ok(output)
    })
    .await;
    match result {
        Ok(Ok(output)) => Ok(output),
        outcome => {
            let _ = child.start_kill();
            let _ = timeout(COMMAND_TIMEOUT, child.wait()).await;
            outcome.unwrap_or(Err("terminal_command_timeout"))
        }
    }
}

#[derive(Deserialize)]
struct WeztermPane {
    pane_id: u64,
    window_id: u64,
    tab_id: u64,
    cwd: String,
    #[serde(default)]
    workspace: String,
    #[serde(default)]
    is_dead: bool,
}

/// File-identity comparison (volume serial + file index on Windows, device +
/// inode elsewhere), the same check the daemon applies to every returned pane.
/// Comparing canonical paths lexically could drop a valid pane whose reported
/// spelling differs in case, which `canonicalize` does not promise to fold.
pub fn same_directory(left: &str, right: &str) -> bool {
    same_file::is_same_file(left, right).unwrap_or(false)
}

pub fn parse_inventory(
    bytes: &[u8],
    expected_cwd: &str,
) -> Result<Vec<TerminalPane>, &'static str> {
    if bytes.len() > MAX_OUTPUT_BYTES {
        return Err("terminal_output_too_large");
    }
    let source: Vec<WeztermPane> =
        serde_json::from_slice(bytes).map_err(|_| "terminal_invalid_inventory")?;
    let mut panes = Vec::new();
    for pane in source {
        if pane.is_dead {
            continue;
        }
        // WezTerm emits file://HOST/path URLs. Reject remote hosts instead of
        // converting them into local paths or UNC network access.
        let cwd = if pane.cwd.starts_with("file:") {
            let mut url = url::Url::parse(&pane.cwd).map_err(|_| "terminal_invalid_inventory")?;
            if let Some(host) = url.host_str() {
                let local_host = sysinfo::System::host_name().unwrap_or_default();
                if !host.eq_ignore_ascii_case("localhost")
                    && !host.eq_ignore_ascii_case(&local_host)
                {
                    continue;
                }
                url.set_host(None)
                    .map_err(|_| "terminal_invalid_inventory")?;
            }
            url.to_file_path()
                .map_err(|_| "terminal_invalid_inventory")?
                .to_string_lossy()
                .into_owned()
        } else {
            pane.cwd
        };
        if !same_directory(&cwd, expected_cwd) {
            continue;
        }
        if panes.len() >= MAX_TERMINAL_PANES {
            return Err("terminal_inventory_capacity");
        }
        panes.push(TerminalPane {
            pane_id: pane.pane_id.to_string(),
            cwd,
            workspace: pane.workspace,
            window_id: pane.window_id.to_string(),
            tab_id: pane.tab_id.to_string(),
        });
    }
    Ok(panes)
}

/// Observe every live pane before selecting directories at or below the root.
/// An unresolved or ambiguous pane makes the snapshot incomplete; this parser
/// neither grants cleanup authority nor changes any filesystem resource.
pub fn parse_cleanup_inventory(
    bytes: &[u8],
    expected_root: &str,
) -> Result<Vec<TerminalPane>, &'static str> {
    if bytes.len() > MAX_OUTPUT_BYTES {
        return Err("terminal_output_too_large");
    }
    let source: Vec<WeztermPane> =
        serde_json::from_slice(bytes).map_err(|_| "terminal_invalid_inventory")?;
    if source.len() > MAX_CLEANUP_SOURCE_PANES {
        return Err("terminal_inventory_capacity");
    }
    if source.iter().enumerate().any(|(index, pane)| {
        source[..index]
            .iter()
            .any(|previous| previous.pane_id == pane.pane_id)
    }) {
        return Err("terminal_invalid_inventory");
    }
    if !valid_label(expected_root, MAX_CWD_BYTES) || !cleanup_local_path(Path::new(expected_root)) {
        return Err("terminal_invalid_cwd");
    }
    let root_metadata =
        std::fs::symlink_metadata(expected_root).map_err(|_| "terminal_cleanup_incomplete")?;
    if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
        return Err("terminal_cleanup_incomplete");
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if root_metadata.file_attributes() & 0x400 != 0 {
            return Err("terminal_cleanup_incomplete");
        }
    }
    let root = cleanup_directory(expected_root)?;
    let root_identity =
        same_file::Handle::from_path(&root).map_err(|_| "terminal_cleanup_incomplete")?;
    let mut panes = Vec::new();
    for pane in source {
        if pane.is_dead {
            continue;
        }
        if !valid_label(&pane.cwd, MAX_CWD_BYTES) {
            return Err("terminal_cleanup_incomplete");
        }
        let cwd = if pane
            .cwd
            .get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file:"))
        {
            let mut url = url::Url::parse(&pane.cwd).map_err(|_| "terminal_cleanup_incomplete")?;
            if !url.username().is_empty()
                || url.password().is_some()
                || url.port().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err("terminal_cleanup_incomplete");
            }
            if let Some(host) = url.host_str() {
                let local_host = sysinfo::System::host_name().unwrap_or_default();
                if !host.eq_ignore_ascii_case("localhost")
                    && (local_host.is_empty() || !host.eq_ignore_ascii_case(&local_host))
                {
                    return Err("terminal_cleanup_incomplete");
                }
                url.set_host(None)
                    .map_err(|_| "terminal_cleanup_incomplete")?;
            }
            url.to_file_path()
                .map_err(|_| "terminal_cleanup_incomplete")?
        } else {
            pane.cwd.into()
        };
        let cwd = cleanup_directory(cwd.to_str().ok_or("terminal_cleanup_incomplete")?)?;
        let mut matches_root = false;
        // Compare actual directory identities on the resolved ancestor chain.
        // Case aliases and junctions cannot be decided by a string prefix.
        for ancestor in cwd.ancestors() {
            let identity = same_file::Handle::from_path(ancestor)
                .map_err(|_| "terminal_cleanup_incomplete")?;
            if identity == root_identity {
                matches_root = true;
                break;
            }
        }
        let observed = TerminalPane {
            pane_id: pane.pane_id.to_string(),
            cwd: cwd
                .to_str()
                .ok_or("terminal_cleanup_incomplete")?
                .to_owned(),
            workspace: pane.workspace,
            window_id: pane.window_id.to_string(),
            tab_id: pane.tab_id.to_string(),
        };
        if !vibemux_plugin_protocol::terminal::valid_pane(&observed) {
            return Err("terminal_invalid_inventory");
        }
        if matches_root {
            if panes.len() >= MAX_TERMINAL_PANES {
                return Err("terminal_inventory_capacity");
            }
            panes.push(observed);
        }
    }
    Ok(panes)
}

fn cleanup_directory(value: &str) -> Result<std::path::PathBuf, &'static str> {
    let path = Path::new(value);
    if !valid_label(value, MAX_CWD_BYTES) || !cleanup_local_path(path) {
        return Err("terminal_cleanup_incomplete");
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| "terminal_cleanup_incomplete")?;
    if !cleanup_local_path(&canonical)
        || !canonical
            .metadata()
            .map_err(|_| "terminal_cleanup_incomplete")?
            .is_dir()
    {
        return Err("terminal_cleanup_incomplete");
    }
    Ok(canonical)
}

fn cleanup_local_path(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        matches!(
            path.components().next(),
            Some(Component::Prefix(prefix))
                if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
        )
    }
    #[cfg(not(windows))]
    {
        // A leading double slash has implementation-defined network semantics.
        !path.as_os_str().as_encoded_bytes().starts_with(b"//")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inventory_filters_cwd_without_returning_terminal_titles() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_string_lossy().into_owned();
        let input = serde_json::json!([
            {"pane_id":1,"window_id":2,"tab_id":3,"cwd":cwd,"title":"PRIVATE PROMPT"},
            {"pane_id":4,"window_id":2,"tab_id":3,"cwd":"missing_directory"}
        ]);
        let panes = parse_inventory(&serde_json::to_vec(&input).unwrap(), &cwd).unwrap();
        assert_eq!(panes.len(), 1);
        assert!(!serde_json::to_string(&panes).unwrap().contains("PRIVATE"));
    }
    #[test]
    fn invalid_and_oversized_inventory_is_rejected() {
        assert!(parse_inventory(b"invalid", "missing").is_err());
        assert!(parse_inventory(&vec![b' '; MAX_OUTPUT_BYTES + 1], "missing").is_err());
        assert!(!same_directory("missing", "missing"));
    }
    #[test]
    fn directory_identity_ignores_path_spelling() {
        let temp = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_string_lossy().into_owned();
        assert!(same_directory(
            &cwd,
            &format!("{cwd}{}.", std::path::MAIN_SEPARATOR)
        ));
        assert!(!same_directory(&cwd, &other.path().to_string_lossy()));
    }
    #[cfg(windows)]
    #[test]
    fn windows_inventory_matches_differently_cased_cwd() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_string_lossy().into_owned();
        let reported = format!(
            "file:///{}",
            cwd.to_uppercase().replace(std::path::MAIN_SEPARATOR, "/")
        );
        let input = serde_json::json!([{"pane_id":1,"window_id":2,"tab_id":3,"cwd":reported}]);
        let panes =
            parse_inventory(&serde_json::to_vec(&input).unwrap(), &cwd.to_lowercase()).unwrap();
        assert_eq!(panes.len(), 1);
    }

    #[test]
    fn cleanup_inventory_includes_descendants_omitted_by_legacy_inventory() {
        let temp = tempfile::tempdir().unwrap();
        let child = temp.path().join("child");
        std::fs::create_dir(&child).unwrap();
        let root = temp.path().to_string_lossy().into_owned();
        let input = serde_json::json!([
            {"pane_id":1,"window_id":2,"tab_id":3,"cwd":root},
            {"pane_id":4,"window_id":2,"tab_id":3,"cwd":child,"title":"PRIVATE PROMPT"}
        ]);
        let bytes = serde_json::to_vec(&input).unwrap();
        assert_eq!(parse_inventory(&bytes, &root).unwrap().len(), 1);
        let panes = parse_cleanup_inventory(&bytes, &root).unwrap();
        assert_eq!(panes.len(), 2);
        assert_eq!(panes[1].pane_id, "4");
        assert!(same_directory(&panes[1].cwd, child.to_str().unwrap()));
        assert!(!serde_json::to_string(&panes).unwrap().contains("PRIVATE"));
    }

    #[test]
    fn cleanup_inventory_rejects_unknown_live_cwd_instead_of_omitting_it() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_str().unwrap();
        let missing = temp.path().join("missing");
        let file = temp.path().join("file");
        std::fs::write(&file, b"fixture").unwrap();
        for unknown in ["", ".", missing.to_str().unwrap(), file.to_str().unwrap()] {
            let input = serde_json::json!([
                {"pane_id":1,"window_id":2,"tab_id":3,"cwd":unknown}
            ]);
            let bytes = serde_json::to_vec(&input).unwrap();
            assert!(parse_inventory(&bytes, root).unwrap().is_empty());
            assert_eq!(
                parse_cleanup_inventory(&bytes, root),
                Err("terminal_cleanup_incomplete")
            );
        }
        let input = serde_json::json!([
            {"pane_id":1,"window_id":2,"tab_id":3,"cwd":root},
            {"pane_id":4,"window_id":2,"tab_id":3,"cwd":missing}
        ]);
        assert_eq!(
            parse_cleanup_inventory(&serde_json::to_vec(&input).unwrap(), root),
            Err("terminal_cleanup_incomplete")
        );
        assert!(!missing.exists());
    }

    #[test]
    fn cleanup_inventory_omits_existing_unrelated_and_prefix_trap_directories() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("worktrees");
        let prefix_trap = temp.path().join("worktrees-evil");
        let unrelated = temp.path().join("other");
        for path in [&root, &prefix_trap, &unrelated] {
            std::fs::create_dir(path).unwrap();
        }
        let input = serde_json::json!([
            {"pane_id":1,"window_id":2,"tab_id":3,"cwd":prefix_trap},
            {"pane_id":4,"window_id":2,"tab_id":3,"cwd":unrelated}
        ]);
        assert!(
            parse_cleanup_inventory(&serde_json::to_vec(&input).unwrap(), root.to_str().unwrap())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn cleanup_inventory_bounds_all_source_panes_including_dead_and_unrelated() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        for dead in [false, true] {
            let source: Vec<_> = (0..=MAX_CLEANUP_SOURCE_PANES)
                .map(|index| {
                    serde_json::json!({
                        "pane_id":index,"window_id":2,"tab_id":3,
                        "cwd":other.path(),"is_dead":dead
                    })
                })
                .collect();
            assert!(
                parse_cleanup_inventory(
                    &serde_json::to_vec(&source[..MAX_CLEANUP_SOURCE_PANES]).unwrap(),
                    root.path().to_str().unwrap()
                )
                .unwrap()
                .is_empty()
            );
            assert_eq!(
                parse_cleanup_inventory(
                    &serde_json::to_vec(&source).unwrap(),
                    root.path().to_str().unwrap()
                ),
                Err("terminal_inventory_capacity")
            );
        }
    }

    #[test]
    fn cleanup_inventory_allows_eight_known_unrelated_panes_and_bounds_matches() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let mut source: Vec<_> = (0..8)
            .map(|index| {
                serde_json::json!({
                    "pane_id":index,"window_id":2,"tab_id":3,"cwd":other.path()
                })
            })
            .collect();
        assert!(
            parse_cleanup_inventory(
                &serde_json::to_vec(&source).unwrap(),
                root.path().to_str().unwrap()
            )
            .unwrap()
            .is_empty()
        );
        for pane in &mut source[..MAX_TERMINAL_PANES + 1] {
            pane["cwd"] = serde_json::json!(root.path());
        }
        assert_eq!(
            parse_cleanup_inventory(
                &serde_json::to_vec(&source).unwrap(),
                root.path().to_str().unwrap()
            ),
            Err("terminal_inventory_capacity")
        );
    }

    #[test]
    fn cleanup_inventory_rejects_duplicate_identifiers_before_filtering() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let input = serde_json::json!([
            {"pane_id":1,"window_id":2,"tab_id":3,"cwd":other.path(),"is_dead":true},
            {"pane_id":1,"window_id":4,"tab_id":5,"cwd":other.path()}
        ]);
        assert_eq!(
            parse_cleanup_inventory(
                &serde_json::to_vec(&input).unwrap(),
                root.path().to_str().unwrap()
            ),
            Err("terminal_invalid_inventory")
        );
    }

    #[test]
    fn cleanup_inventory_requires_valid_bounded_json_and_fields() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_str().unwrap();
        for input in [
            b"invalid".as_slice(),
            b"{}",
            b"null",
            br#"[{"pane_id":1,"window_id":2,"tab_id":3}]"#,
            br#"[{"pane_id":1,"window_id":2,"tab_id":3,"cwd":null}]"#,
        ] {
            assert_eq!(
                parse_cleanup_inventory(input, root),
                Err("terminal_invalid_inventory")
            );
        }
        assert_eq!(
            parse_cleanup_inventory(&vec![b' '; MAX_OUTPUT_BYTES + 1], root),
            Err("terminal_output_too_large")
        );
        let input = serde_json::json!([
            {"pane_id":1,"window_id":2,"tab_id":3,"cwd":root,"workspace":"x".repeat(257)}
        ]);
        assert_eq!(
            parse_cleanup_inventory(&serde_json::to_vec(&input).unwrap(), root),
            Err("terminal_invalid_inventory")
        );
    }

    #[test]
    fn cleanup_inventory_requires_an_existing_ordinary_local_root() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing");
        let file = temp.path().join("file");
        std::fs::write(&file, b"fixture").unwrap();
        for root in [missing.to_str().unwrap(), file.to_str().unwrap()] {
            assert_eq!(
                parse_cleanup_inventory(b"[]", root),
                Err("terminal_cleanup_incomplete")
            );
        }
        for root in ["", ".", r"\\server\share\path", "//server/share/path"] {
            assert_eq!(
                parse_cleanup_inventory(b"[]", root),
                Err("terminal_invalid_cwd")
            );
        }
    }

    #[test]
    fn cleanup_inventory_accepts_local_file_urls_with_encoded_path_characters() {
        let temp = tempfile::tempdir().unwrap();
        let child = temp.path().join("child with spaces ü");
        std::fs::create_dir(&child).unwrap();
        let local_url = url::Url::from_directory_path(&child).unwrap();
        for cwd in [
            local_url.to_string(),
            format!("file://localhost{}", local_url.path()),
        ] {
            let input = serde_json::json!([
                {"pane_id":1,"window_id":2,"tab_id":3,"cwd":cwd}
            ]);
            let panes = parse_cleanup_inventory(
                &serde_json::to_vec(&input).unwrap(),
                temp.path().to_str().unwrap(),
            )
            .unwrap();
            assert_eq!(panes.len(), 1);
            assert!(same_directory(&panes[0].cwd, child.to_str().unwrap()));
        }
    }

    #[test]
    fn cleanup_inventory_rejects_remote_or_ambiguous_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_str().unwrap();
        let local_url = url::Url::from_directory_path(temp.path()).unwrap();
        for cwd in [
            "file://foreign.invalid/share/directory".to_owned(),
            "file://foreign.invalid.invalid/share/directory".to_owned(),
            r"\\server\share\path".to_owned(),
            "//server/share/path".to_owned(),
            r"\\?\UNC\server\share\path".to_owned(),
            r"\\.\pipe\path".to_owned(),
            format!("{local_url}?query=ambiguous"),
            format!("{local_url}#fragment"),
        ] {
            let input = serde_json::json!([
                {"pane_id":1,"window_id":2,"tab_id":3,"cwd":cwd}
            ]);
            assert_eq!(
                parse_cleanup_inventory(&serde_json::to_vec(&input).unwrap(), root),
                Err("terminal_cleanup_incomplete")
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_cleanup_inventory_matches_differently_cased_descendant_cwd() {
        let temp = tempfile::tempdir().unwrap();
        let child = temp.path().join("child");
        std::fs::create_dir(&child).unwrap();
        let cwd = format!(
            "file:///{}",
            child
                .to_str()
                .unwrap()
                .to_uppercase()
                .replace(std::path::MAIN_SEPARATOR, "/")
        );
        let input = serde_json::json!([
            {"pane_id":1,"window_id":2,"tab_id":3,"cwd":cwd}
        ]);
        let panes = parse_cleanup_inventory(
            &serde_json::to_vec(&input).unwrap(),
            &temp.path().to_str().unwrap().to_lowercase(),
        )
        .unwrap();
        assert_eq!(panes.len(), 1);
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn cleanup_inventory_follows_cwd_directory_identity_and_refuses_alias_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let child = root.join("child");
        let alias = temp.path().join("alias");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::create_dir(&outside).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&root, &alias).unwrap();
        let input = serde_json::json!([
            {"pane_id":1,"window_id":2,"tab_id":3,"cwd":alias.join("child")}
        ]);
        let panes =
            parse_cleanup_inventory(&serde_json::to_vec(&input).unwrap(), root.to_str().unwrap())
                .unwrap();
        assert_eq!(panes.len(), 1);
        assert!(same_directory(&panes[0].cwd, child.to_str().unwrap()));
        assert_eq!(
            parse_cleanup_inventory(b"[]", alias.to_str().unwrap()),
            Err("terminal_cleanup_incomplete")
        );
        let input = serde_json::json!([
            {"pane_id":1,"window_id":2,"tab_id":3,"cwd":alias}
        ]);
        assert!(
            parse_cleanup_inventory(
                &serde_json::to_vec(&input).unwrap(),
                outside.to_str().unwrap()
            )
            .unwrap()
            .is_empty()
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_cleanup_inventory_resolves_junction_cwd_and_refuses_junction_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let child = root.join("child");
        let alias = temp.path().join("alias");
        std::fs::create_dir_all(&child).unwrap();
        let system_root = std::env::var_os("SystemRoot").expect("SystemRoot");
        let powershell =
            Path::new(&system_root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let status = Command::new(powershell)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:VIBEMUX_TEST_LINK -Target $env:VIBEMUX_TEST_TARGET | Out-Null",
            ])
            .env("VIBEMUX_TEST_LINK", &alias)
            .env("VIBEMUX_TEST_TARGET", &root)
            .creation_flags(0x0800_0000)
            .status()
            .await
            .expect("junction fixture");
        assert!(status.success(), "junction fixture must exist");
        let input = serde_json::json!([
            {"pane_id":1,"window_id":2,"tab_id":3,"cwd":alias.join("child")}
        ]);
        let panes =
            parse_cleanup_inventory(&serde_json::to_vec(&input).unwrap(), root.to_str().unwrap())
                .unwrap();
        assert_eq!(panes.len(), 1);
        assert!(same_directory(&panes[0].cwd, child.to_str().unwrap()));
        assert_eq!(
            parse_cleanup_inventory(b"[]", alias.to_str().unwrap()),
            Err("terminal_cleanup_incomplete")
        );
        std::fs::remove_dir(&alias).expect("remove fixture junction only");
        assert!(child.is_dir());
    }
}
