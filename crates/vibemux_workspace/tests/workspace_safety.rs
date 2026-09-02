use std::{
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::process::Command;
use vibemux_types::RunId;
use vibemux_workspace::WorkspaceManager;

async fn git(executable: &Path, root: &Path, arguments: &[&str]) -> String {
    let mut command = Command::new(executable);
    command
        .current_dir(root)
        .arg("-c")
        .arg("user.name=VibeMux Test")
        .arg("-c")
        .arg("user.email=fixture@example.invalid")
        .arg("-c")
        .arg(if cfg!(windows) {
            "core.hooksPath=NUL"
        } else {
            "core.hooksPath=/dev/null"
        })
        .arg("-c")
        .arg("commit.gpgSign=false")
        .args(arguments)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .env_clear()
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        );
    for key in ["SystemRoot", "WINDIR", "TEMP", "TMP", "PATH"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let output = command.output().await.expect("Git fixture command");
    assert!(
        output.status.success(),
        "fixture git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("Git UTF8")
}

fn git_executable() -> PathBuf {
    let name = if cfg!(windows) { "git.exe" } else { "git" };
    std::env::split_paths(&std::env::var_os("PATH").expect("PATH"))
        .map(|directory| directory.join(name))
        .find(|path| path.is_file())
        .and_then(|path| std::fs::canonicalize(path).ok())
        .expect("Git executable")
}

async fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, String) {
    let directory = tempfile::tempdir().expect("temporary repository");
    let repo = directory.path().join("repo with spaces_状态");
    std::fs::create_dir(&repo).expect("repository directory");
    let executable = git_executable();
    git(
        &executable,
        &repo,
        &["init", "--quiet", "--initial-branch=main"],
    )
    .await;
    std::fs::write(repo.join("base.txt"), "unchanged base\n").expect("base fixture");
    std::fs::write(repo.join(".gitignore"), ".vibemux/\nverification.json\n")
        .expect("ignore managed state");
    git(&executable, &repo, &["add", "base.txt", ".gitignore"]).await;
    git(
        &executable,
        &repo,
        &["commit", "--quiet", "-m", "fixture initial commit"],
    )
    .await;
    let base = git(&executable, &repo, &["rev-parse", "HEAD"])
        .await
        .trim()
        .to_string();
    (directory, executable, repo, base)
}

#[tokio::test]
async fn forged_owner_token_cannot_authorize_existing_git_worktree() {
    let (_directory, executable, repo, base) = fixture().await;
    let manager = WorkspaceManager::new(executable, repo, base)
        .await
        .expect("manager");
    let mut record = manager.create(RunId::new()).await.expect("worktree");
    let original = record.clone();
    record.ownership_token = "forged_owner".to_string();
    assert!(
        manager.inspect(&record).await.is_err(),
        "an arbitrary owner token must not authorize a real worktree"
    );
    manager
        .cleanup(&original)
        .await
        .expect("valid owner cleanup");
}

#[tokio::test]
async fn real_separate_worktrees_keep_main_head_index_config_and_files_unchanged() {
    let (_directory, executable, repo, base) = fixture().await;
    let main_head = std::fs::read(repo.join(".git/HEAD")).expect("main HEAD");
    let main_index = std::fs::read(repo.join(".git/index")).expect("main index");
    let main_config = std::fs::read(repo.join(".git/config")).expect("main config");
    // Git would fail worktree creation if this post-checkout hook were invoked.
    let hook = repo.join(".git/hooks/post-checkout");
    std::fs::write(&hook, "#!/bin/sh\nexit 79\n").expect("owned fixture hook");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))
            .expect("executable hook");
    }
    let manager = WorkspaceManager::new(executable.clone(), repo.clone(), base.clone())
        .await
        .expect("manager");
    let first = manager.create(RunId::new()).await.expect("first run");
    let second = manager.create(RunId::new()).await.expect("second run");
    assert_ne!(first.path, second.path);
    assert_ne!(first.branch, second.branch);
    assert_ne!(first.ownership_token, second.ownership_token);
    manager.inspect(&first).await.expect("first owned");
    manager.inspect(&second).await.expect("second owned");
    let git_dir = git(
        &executable,
        Path::new(&first.path),
        &["rev-parse", "--absolute-git-dir"],
    )
    .await;
    assert!(
        Path::new(git_dir.trim())
            .join("vibemux_owner.json")
            .is_file()
    );
    assert!(!Path::new(&first.path).join("vibemux_owner.json").exists());
    assert_eq!(
        std::fs::read(repo.join(".git/HEAD")).expect("HEAD"),
        main_head
    );
    assert_eq!(
        std::fs::read(repo.join(".git/index")).expect("index"),
        main_index
    );
    assert_eq!(
        std::fs::read(repo.join(".git/config")).expect("config"),
        main_config
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("base.txt")).expect("base"),
        "unchanged base\n"
    );
    assert!(
        git(&executable, &repo, &["status", "--porcelain"])
            .await
            .is_empty()
    );
    manager.cleanup(&first).await.expect("owned clean removal");
    manager
        .cleanup(&second)
        .await
        .expect("second clean removal");
    assert!(!Path::new(&first.path).exists());
    assert!(!Path::new(&second.path).exists());
    assert_eq!(
        git(&executable, &repo, &["rev-parse", &first.branch])
            .await
            .trim(),
        base
    );
    assert_eq!(
        git(&executable, &repo, &["rev-parse", "HEAD"]).await.trim(),
        base
    );
}

#[tokio::test]
async fn artifacts_are_immutable_bounded_and_ignored_artifacts_block_cleanup() {
    let (_directory, executable, repo, base) = fixture().await;
    let manager = WorkspaceManager::new(executable, repo, base)
        .await
        .expect("manager");
    let record = manager.create(RunId::new()).await.expect("run");
    let reference = manager
        .write_artifact(&record, "result.json", &serde_json::json!({"answer":42}))
        .await
        .expect("artifact");
    let original =
        std::fs::read(Path::new(&record.path).join("result.json")).expect("written result");
    assert_eq!(reference.sha256, vibemux_workspace::sha256(&original));
    assert_eq!(reference.size_bytes, original.len() as u64);
    assert!(
        manager
            .write_artifact(&record, "result.json", &serde_json::json!({"answer":0}))
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(Path::new(&record.path).join("result.json")).expect("unchanged result"),
        original
    );
    assert!(
        manager
            .write_artifact(&record, "../escape.json", &true)
            .await
            .is_err()
    );
    assert!(
        manager
            .write_artifact(&record, "review.json", &"x".repeat(40 * 1024))
            .await
            .is_err()
    );
    assert!(!Path::new(&record.path).join("review.json").exists());
    assert!(manager.plan_cleanup(&record).await.is_err());
    assert!(manager.cleanup(&record).await.is_err());
    std::fs::remove_file(Path::new(&record.path).join("result.json"))
        .expect("remove exact test artifact");
    manager
        .write_artifact(&record, "verification.json", &true)
        .await
        .expect("ignored artifact");
    assert!(
        manager.cleanup(&record).await.is_err(),
        "ignored output remains user data"
    );
    std::fs::remove_file(Path::new(&record.path).join("verification.json"))
        .expect("remove exact ignored test artifact");
    manager
        .cleanup(&record)
        .await
        .expect("cleanup after explicit fixture removal");
}

#[tokio::test]
async fn owner_receipt_mismatch_missing_and_oversized_never_authorize_mutation() {
    let (_directory, executable, repo, base) = fixture().await;
    let manager = WorkspaceManager::new(executable.clone(), repo, base)
        .await
        .expect("manager");
    let record = manager.create(RunId::new()).await.expect("run");
    let git_dir = git(
        &executable,
        Path::new(&record.path),
        &["rev-parse", "--absolute-git-dir"],
    )
    .await;
    let receipt_path = Path::new(git_dir.trim()).join("vibemux_owner.json");
    let original = std::fs::read(&receipt_path).expect("owner receipt");
    for contents in [b"{}".to_vec(), vec![b' '; 20 * 1024]] {
        std::fs::write(&receipt_path, contents).expect("corrupt owned fixture receipt");
        assert!(manager.inspect(&record).await.is_err());
        assert!(
            manager
                .write_artifact(&record, "result.json", &true)
                .await
                .is_err()
        );
        assert!(manager.cleanup(&record).await.is_err());
        assert!(Path::new(&record.path).exists());
        assert!(!Path::new(&record.path).join("result.json").exists());
    }
    std::fs::remove_file(&receipt_path).expect("remove exact fixture receipt");
    assert!(manager.inspect(&record).await.is_err());
    std::fs::write(&receipt_path, original).expect("restore exact original receipt");
    manager.cleanup(&record).await.expect("valid owner cleanup");
}

#[tokio::test]
async fn prefix_traps_main_missing_and_unregistered_repositories_are_refused() {
    let (_directory, executable, repo, base) = fixture().await;
    let manager = WorkspaceManager::new(executable.clone(), repo.clone(), base.clone())
        .await
        .expect("manager");
    let record = manager.create(RunId::new()).await.expect("run");
    let original = record.clone();
    let prefix = repo
        .join(".vibemux/worktrees_evil")
        .join(RunId::new().to_string());
    std::fs::create_dir_all(&prefix).expect("owned prefix trap directory");
    for path in [
        std::fs::canonicalize(&repo).expect("root"),
        std::fs::canonicalize(&prefix).expect("prefix"),
        repo.join("missing_run"),
    ] {
        let mut record = original.clone();
        record.path = path.to_string_lossy().into_owned();
        assert!(manager.inspect(&record).await.is_err());
        assert!(manager.cleanup(&record).await.is_err());
    }
    let fake_id = RunId::new();
    let fake = repo.join(".vibemux/worktrees").join(fake_id.to_string());
    std::fs::create_dir(&fake).expect("owned fake repository directory");
    let fake_branch = format!("codex/run_{}", fake_id.as_uuid().simple());
    git(
        &executable,
        &fake,
        &[
            "init",
            "--quiet",
            &format!("--initial-branch={fake_branch}"),
        ],
    )
    .await;
    let fake_record = vibemux_types::a2a::RunWorkspace {
        path: std::fs::canonicalize(&fake)
            .expect("fake canonical")
            .to_string_lossy()
            .into_owned(),
        branch: fake_branch,
        base_commit: base,
        ownership_token: "invented_owner".to_string(),
    };
    assert!(
        manager.inspect(&fake_record).await.is_err(),
        "nested Git repository is absent from authoritative inventory"
    );
    assert!(
        manager
            .write_artifact(&fake_record, "result.json", &true)
            .await
            .is_err()
    );
    assert!(fake.is_dir());
    git(
        &executable,
        Path::new(&record.path),
        &["symbolic-ref", "HEAD", "refs/heads/unknown_branch"],
    )
    .await;
    assert!(manager.inspect(&record).await.is_err());
    git(
        &executable,
        Path::new(&record.path),
        &[
            "symbolic-ref",
            "HEAD",
            &format!("refs/heads/{}", record.branch),
        ],
    )
    .await;
    manager
        .cleanup(&record)
        .await
        .expect("original identity restored");
}

#[tokio::test]
async fn managed_directory_symlink_or_junction_is_refused_without_touching_target() {
    let (directory, executable, repo, base) = fixture().await;
    let target = directory.path().join("outside_managed");
    std::fs::create_dir(&target).expect("outside target");
    std::fs::write(target.join("sentinel.txt"), "untouched").expect("sentinel");
    let link = repo.join(".vibemux");
    #[cfg(windows)]
    {
        let powershell = PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"))
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let status = Command::new(powershell).args(["-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:VIBEMUX_TEST_LINK -Target $env:VIBEMUX_TEST_TARGET | Out-Null"])
            .env("VIBEMUX_TEST_LINK", &link).env("VIBEMUX_TEST_TARGET", &target).creation_flags(0x0800_0000).status().await.expect("junction fixture");
        assert!(
            status.success(),
            "junction fixture must exist for a meaningful platform test"
        );
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &link).expect("symlink fixture");
    assert!(WorkspaceManager::new(executable, repo, base).await.is_err());
    assert_eq!(
        std::fs::read_to_string(target.join("sentinel.txt")).expect("sentinel"),
        "untouched"
    );
    assert!(!target.join("worktrees").exists());
    #[cfg(windows)]
    std::fs::remove_dir(link).expect("remove junction only");
    #[cfg(unix)]
    std::fs::remove_file(link).expect("remove symlink only");
}
