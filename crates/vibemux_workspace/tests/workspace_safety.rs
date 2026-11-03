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
async fn cleanup_observation_is_inert_clean_and_tracks_committed_head() {
    let (_directory, executable, repo, base) = fixture().await;
    let manager = WorkspaceManager::new(executable.clone(), repo.clone(), base.clone())
        .await
        .expect("manager");
    let record = manager.create(RunId::new()).await.expect("owned worktree");
    let before_inventory = git(&executable, &repo, &["worktree", "list", "--porcelain"]).await;
    let before_head = std::fs::read(repo.join(".git/HEAD")).expect("main HEAD");
    let observed = manager
        .observe_cleanup(&record)
        .await
        .expect("clean observation");
    assert_eq!(
        observed.repository_root,
        std::fs::canonicalize(&repo).expect("root")
    );
    assert_eq!(
        observed.common_git_dir,
        std::fs::canonicalize(repo.join(".git")).expect("common")
    );
    assert_eq!(observed.head_commit, base);
    let debug = format!("{observed:?}");
    assert!(!debug.contains(&record.path));
    assert!(!debug.contains(&observed.repository_root.to_string_lossy().to_string()));
    assert_eq!(
        git(&executable, &repo, &["worktree", "list", "--porcelain"]).await,
        before_inventory
    );
    assert_eq!(
        std::fs::read(repo.join(".git/HEAD")).expect("main HEAD"),
        before_head
    );

    let tracked = Path::new(&record.path).join("base.txt");
    std::fs::write(&tracked, "owned dirty change\n").expect("dirty tracked fixture");
    assert!(manager.observe_cleanup(&record).await.is_err());
    std::fs::write(&tracked, "unchanged base\n").expect("restore fixture bytes");
    let untracked = Path::new(&record.path).join("untracked.txt");
    std::fs::write(&untracked, "owned untracked data\n").expect("untracked fixture");
    assert!(manager.observe_cleanup(&record).await.is_err());
    std::fs::remove_file(&untracked).expect("remove exact fixture file");
    let ignored = Path::new(&record.path).join("verification.json");
    std::fs::write(&ignored, "owned ignored data\n").expect("ignored fixture");
    assert!(manager.observe_cleanup(&record).await.is_err());
    std::fs::remove_file(&ignored).expect("remove exact ignored fixture");

    git(
        &executable,
        Path::new(&record.path),
        &[
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "owned committed work",
        ],
    )
    .await;
    let committed = git(&executable, Path::new(&record.path), &["rev-parse", "HEAD"])
        .await
        .trim()
        .to_owned();
    assert_ne!(committed, base);
    assert_eq!(
        manager
            .observe_cleanup(&record)
            .await
            .expect("committed clean work remains eligible")
            .head_commit,
        committed
    );
    manager
        .cleanup(&record)
        .await
        .expect("only owned fixture removed");
    assert_eq!(
        git(&executable, &repo, &["rev-parse", "HEAD"]).await.trim(),
        base
    );
    assert!(
        git(&executable, &repo, &["status", "--porcelain"])
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn a_git_moved_worktree_is_not_observed_as_removed() {
    let (_directory, executable, repo, base) = fixture().await;
    let manager = WorkspaceManager::new(executable.clone(), repo.clone(), base.clone())
        .await
        .expect("manager");
    let record = manager.create(RunId::new()).await.expect("owned worktree");
    let main_head = std::fs::read(repo.join(".git/HEAD")).expect("main HEAD");
    let main_index = std::fs::read(repo.join(".git/index")).expect("main index");
    let main_config = std::fs::read(repo.join(".git/config")).expect("main config");
    let moved = PathBuf::from(&record.path).with_extension("git_moved_fixture");
    assert_eq!(moved.parent(), Path::new(&record.path).parent());
    let original_argument = record.path.strip_prefix("\\\\?\\").unwrap_or(&record.path);
    let moved_text = moved.to_str().expect("owned moved path");
    let moved_argument = moved_text.strip_prefix("\\\\?\\").unwrap_or(moved_text);
    git(
        &executable,
        &repo,
        &["worktree", "move", "--", original_argument, moved_argument],
    )
    .await;
    assert!(!Path::new(&record.path).exists());
    assert!(moved.is_dir());
    assert_eq!(
        git(&executable, &moved, &["symbolic-ref", "--short", "HEAD"])
            .await
            .trim(),
        record.branch
    );
    let observed = manager.observe_removed(&record, &base).await;
    #[cfg(windows)]
    let case_alias_observed = {
        git(
            &executable,
            &moved,
            &[
                "symbolic-ref",
                "HEAD",
                &format!("refs/heads/{}", record.branch.to_ascii_uppercase()),
            ],
        )
        .await;
        let observed = manager.observe_removed(&record, &base).await;
        git(
            &executable,
            &moved,
            &[
                "symbolic-ref",
                "HEAD",
                &format!("refs/heads/{}", record.branch),
            ],
        )
        .await;
        observed
    };
    // Restore Git's binding before managed cleanup, including on a regression.
    git(
        &executable,
        &repo,
        &["worktree", "move", "--", moved_argument, original_argument],
    )
    .await;
    manager
        .cleanup(&record)
        .await
        .expect("restored owned binding cleanup");
    assert_eq!(
        std::fs::read(repo.join(".git/HEAD")).expect("main HEAD"),
        main_head
    );
    assert_eq!(
        std::fs::read(repo.join(".git/index")).expect("main index"),
        main_index
    );
    assert_eq!(
        std::fs::read(repo.join(".git/config")).expect("main config"),
        main_config
    );
    assert_eq!(
        git(&executable, &repo, &["rev-parse", "HEAD"]).await.trim(),
        base
    );
    assert!(
        git(&executable, &repo, &["status", "--porcelain"])
            .await
            .is_empty()
    );
    assert!(
        !observed.expect("moved worktree observation"),
        "the Run branch remains attached at another path"
    );
    #[cfg(windows)]
    assert!(
        !case_alias_observed.expect("case alias observation"),
        "Windows branch aliases remain attached too"
    );
}

#[tokio::test]
async fn removal_observation_requires_absent_slot_inventory_and_retained_branch_head() {
    let (_directory, executable, repo, base) = fixture().await;
    let manager = WorkspaceManager::new(executable.clone(), repo.clone(), base.clone())
        .await
        .expect("manager");
    let record = manager.create(RunId::new()).await.expect("owned worktree");
    let main_head = std::fs::read(repo.join(".git/HEAD")).expect("main HEAD");
    let main_index = std::fs::read(repo.join(".git/index")).expect("main index");
    let main_config = std::fs::read(repo.join(".git/config")).expect("main config");
    assert!(
        !manager
            .observe_removed(&record, &base)
            .await
            .expect("still present")
    );
    for invalid in ["", "HEAD", "-bad", &"z".repeat(40), &"a".repeat(39)] {
        assert!(manager.observe_removed(&record, invalid).await.is_err());
    }
    let moved = PathBuf::from(&record.path).with_extension("owned_moved_fixture");
    assert_eq!(moved.parent(), Path::new(&record.path).parent());
    std::fs::rename(&record.path, &moved).expect("move exact owned temporary worktree");
    assert!(
        !manager
            .observe_removed(&record, &base)
            .await
            .expect("stale inventory retains target")
    );
    std::fs::rename(&moved, &record.path).expect("restore exact owned temporary worktree");
    manager.cleanup(&record).await.expect("owned clean removal");
    assert!(
        manager
            .observe_removed(&record, &base)
            .await
            .expect("positive observed postconditions")
    );
    assert_eq!(
        git(&executable, &repo, &["rev-parse", &record.branch])
            .await
            .trim(),
        base
    );

    let mut forged = record.clone();
    forged.path = repo.to_string_lossy().into_owned();
    assert!(manager.observe_removed(&forged, &base).await.is_err());
    std::fs::create_dir(&record.path).expect("unrelated replacement at old slot");
    std::fs::write(Path::new(&record.path).join("sentinel.txt"), "preserved\n")
        .expect("replacement data");
    assert!(
        !manager
            .observe_removed(&record, &base)
            .await
            .expect("old path reappeared")
    );
    assert_eq!(
        std::fs::read_to_string(Path::new(&record.path).join("sentinel.txt")).expect("sentinel"),
        "preserved\n"
    );
    std::fs::remove_file(Path::new(&record.path).join("sentinel.txt"))
        .expect("remove exact fixture file");
    std::fs::remove_dir(&record.path).expect("remove exact empty replacement fixture");

    let advanced = git(
        &executable,
        &repo,
        &[
            "commit-tree",
            &format!("{base}^{{tree}}"),
            "-p",
            &base,
            "-m",
            "unreferenced fixture commit",
        ],
    )
    .await
    .trim()
    .to_owned();
    git(
        &executable,
        &repo,
        &[
            "update-ref",
            &format!("refs/heads/{}", record.branch),
            &advanced,
        ],
    )
    .await;
    assert!(
        !manager
            .observe_removed(&record, &base)
            .await
            .expect("branch moved")
    );
    git(
        &executable,
        &repo,
        &[
            "update-ref",
            &format!("refs/heads/{}", record.branch),
            &base,
        ],
    )
    .await;
    assert!(
        manager
            .observe_removed(&record, &base)
            .await
            .expect("approved branch restored")
    );
    git(
        &executable,
        &repo,
        &["update-ref", "-d", &format!("refs/heads/{}", record.branch)],
    )
    .await;
    assert!(
        manager.observe_removed(&record, &base).await.is_err(),
        "missing retained branch is not removal success"
    );
    assert!(
        git(&executable, &repo, &["status", "--porcelain"])
            .await
            .is_empty()
    );
    assert_eq!(
        std::fs::read(repo.join(".git/HEAD")).expect("main HEAD"),
        main_head
    );
    assert_eq!(
        std::fs::read(repo.join(".git/index")).expect("main index"),
        main_index
    );
    assert_eq!(
        std::fs::read(repo.join(".git/config")).expect("main config"),
        main_config
    );
    assert_eq!(
        git(&executable, &repo, &["rev-parse", "HEAD"]).await.trim(),
        base
    );
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

fn managed_worktrees_root(repo: &Path) -> PathBuf {
    std::fs::canonicalize(repo.join(".vibemux/worktrees")).expect("canonical managed root")
}

#[tokio::test]
async fn plan_is_deterministic_and_has_no_git_or_worktree_effect() {
    let (_directory, executable, repo, base) = fixture().await;
    let manager = WorkspaceManager::new(executable.clone(), repo.clone(), base.clone())
        .await
        .expect("manager");
    let run_id = RunId::new();
    let other_run_id = RunId::new();
    let plan = manager.plan(run_id).expect("plan");
    assert_eq!(
        plan.path,
        managed_worktrees_root(&repo)
            .join(run_id.to_string())
            .to_string_lossy()
            .into_owned()
    );
    assert_eq!(
        plan.branch,
        format!("codex/run_{}", run_id.as_uuid().simple())
    );
    assert_eq!(plan.base_commit, base);
    assert_eq!(plan.ownership_token.len(), 32);
    assert!(
        plan.ownership_token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    );
    assert_ne!(
        plan.path,
        manager.plan(other_run_id).expect("other plan").path
    );
    // A fresh plan for the same run keeps the deterministic binding but mints
    // a fresh ownership token.
    let replanned = manager.plan(run_id).expect("replan");
    assert_eq!(replanned.path, plan.path);
    assert_eq!(replanned.branch, plan.branch);
    assert_ne!(replanned.ownership_token, plan.ownership_token);
    // Planning had no Git or worktree effect.
    assert!(!Path::new(&plan.path).exists());
    assert!(!Path::new(&replanned.path).exists());
    assert_eq!(
        git(
            &executable,
            &repo,
            &["for-each-ref", "--format=%(refname)", "refs/heads"]
        )
        .await
        .trim(),
        "refs/heads/main"
    );
    assert_eq!(
        git(&executable, &repo, &["worktree", "list", "--porcelain"])
            .await
            .lines()
            .filter(|line| line.starts_with("worktree "))
            .count(),
        1,
        "only the main worktree is registered"
    );
    assert!(
        git(&executable, &repo, &["status", "--porcelain"])
            .await
            .is_empty()
    );
    // The planned run still creates cleanly afterwards.
    let created = manager.create(run_id).await.expect("create after planning");
    assert_eq!(created.path, plan.path);
    assert_eq!(created.branch, plan.branch);
    manager
        .cleanup(&created)
        .await
        .expect("planned run cleanup");
}

#[tokio::test]
async fn planned_creation_round_trips_the_persisted_record_and_recovers_after_restart() {
    let (_directory, executable, repo, base) = fixture().await;
    let manager = WorkspaceManager::new(executable.clone(), repo.clone(), base.clone())
        .await
        .expect("manager");
    let run_id = RunId::new();
    let plan = manager.plan(run_id).expect("plan");
    // The parent writer persists this plan before any effect; the exact
    // persisted record — including the planned ownership token — is executed.
    let persisted = plan.clone();
    let created = manager
        .create_planned(run_id, &persisted)
        .await
        .expect("planned worktree");
    assert_eq!(
        created, persisted,
        "created record equals the persisted plan"
    );
    manager
        .inspect(&persisted)
        .await
        .expect("inspect recovers the matching completed creation");
    let mut forged_token = persisted.clone();
    forged_token.ownership_token = "invented_owner".to_string();
    assert!(
        manager.inspect(&forged_token).await.is_err(),
        "the receipt must bind the supplied planned token"
    );
    // A fresh manager over the same repository reconciles the same record.
    let restarted = WorkspaceManager::new(executable.clone(), repo.clone(), base.clone())
        .await
        .expect("restarted manager");
    restarted
        .inspect(&persisted)
        .await
        .expect("restarted manager reconciles the completed creation");
    // Duplicate creation refuses, including the exact matching record.
    assert!(manager.create_planned(run_id, &persisted).await.is_err());
    assert!(manager.create(run_id).await.is_err());
    manager
        .inspect(&persisted)
        .await
        .expect("refused duplicates left the workspace untouched");
    manager
        .cleanup(&persisted)
        .await
        .expect("valid owner cleanup");
    assert_eq!(
        git(&executable, &repo, &["rev-parse", "HEAD"]).await.trim(),
        base
    );
}

#[tokio::test]
async fn forged_or_mismatched_plans_are_refused_before_any_effect() {
    let (_directory, executable, repo, base) = fixture().await;
    let manager = WorkspaceManager::new(executable.clone(), repo.clone(), base.clone())
        .await
        .expect("manager");
    let run_id = RunId::new();
    let other_run_id = RunId::new();
    let plan = manager.plan(run_id).expect("plan");
    let mut forged_path = plan.clone();
    forged_path.path = managed_worktrees_root(&repo)
        .join(other_run_id.to_string())
        .to_string_lossy()
        .into_owned();
    let mut forged_branch = plan.clone();
    forged_branch.branch = format!("codex/run_{}", other_run_id.as_uuid().simple());
    let mut forged_base = plan.clone();
    forged_base.base_commit = "0".repeat(40);
    let mut forged_token_shape = plan.clone();
    forged_token_shape.ownership_token = "forged owner!".to_string();
    for record in [
        &forged_path,
        &forged_branch,
        &forged_base,
        &forged_token_shape,
    ] {
        assert!(
            manager.create_planned(run_id, record).await.is_err(),
            "forged record must be refused"
        );
    }
    assert!(
        manager.create_planned(other_run_id, &plan).await.is_err(),
        "record bound to a different run must be refused"
    );
    // Refusals happened before any effect: no directories, no branches, no
    // registered worktrees, and the main repository is intact.
    assert!(!Path::new(&plan.path).exists());
    assert!(!Path::new(&forged_path.path).exists());
    assert_eq!(
        git(
            &executable,
            &repo,
            &["for-each-ref", "--format=%(refname)", "refs/heads"]
        )
        .await
        .trim(),
        "refs/heads/main"
    );
    assert_eq!(
        git(&executable, &repo, &["worktree", "list", "--porcelain"])
            .await
            .lines()
            .filter(|line| line.starts_with("worktree "))
            .count(),
        1
    );
    assert!(
        git(&executable, &repo, &["status", "--porcelain"])
            .await
            .is_empty()
    );
    assert_eq!(
        git(&executable, &repo, &["rev-parse", "HEAD"]).await.trim(),
        base
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("base.txt")).expect("base"),
        "unchanged base\n"
    );
    // The honest plan still executes cleanly after the refusals.
    let created = manager
        .create_planned(run_id, &plan)
        .await
        .expect("honest plan executes");
    assert_eq!(created, plan);
    manager
        .cleanup(&created)
        .await
        .expect("valid owner cleanup");
}
