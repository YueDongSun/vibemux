//! Reads the project `HEAD` commit from Git metadata for the Run's
//! `base_commit` (ADR 029 §2). No Git process runs: the daemon has no trusted
//! Git path, and admission must not depend on one.
//!
//! Supported layouts: a `.git` directory, or a `.git` file whose `gitdir:`
//! line names a linked worktree's directory with an optional `commondir`.
//! `HEAD` is either a detached object id or `ref: refs/...`, resolved through
//! loose refs and then `packed-refs`. Anything else (reftable, an unborn
//! branch, a malformed or oversized file) fails closed with
//! [`DispatchError::WorkingDirectoryInvalid`]; the error never carries a
//! path or file content.

use std::{
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

use vibemux_harness::dispatch::DispatchError;

/// Bound for `.git`, `HEAD`, `commondir`, and loose ref files.
const MAX_METADATA_BYTES: u64 = 4 * 1024;
/// Bound for `packed-refs`.
const MAX_PACKED_REFS_BYTES: u64 = 16 * 1024 * 1024;
const MAX_REF_NAME_BYTES: usize = 1024;
/// Symbolic refs followed before giving up.
const MAX_SYMBOLIC_REF_DEPTH: usize = 5;
const GIT_DIR_PREFIX: &str = "gitdir: ";
const SYMBOLIC_REF_PREFIX: &str = "ref: ";

/// The commit `HEAD` names under `project_root`, as lowercase hex.
pub(super) fn read_head_commit(project_root: &Path) -> Result<String, DispatchError> {
    let git_dir = git_dir(project_root)?;
    let common_dir = common_dir(&git_dir)?;
    let mut target = read_line(&git_dir.join("HEAD"), MAX_METADATA_BYTES)?;
    for _ in 0..=MAX_SYMBOLIC_REF_DEPTH {
        let Some(name) = target.strip_prefix(SYMBOLIC_REF_PREFIX) else {
            return object_id(&target);
        };
        let name = ref_name(name)?.to_string();
        target = resolve_ref(&git_dir, &common_dir, &name)?;
    }
    Err(invalid())
}

fn git_dir(project_root: &Path) -> Result<PathBuf, DispatchError> {
    let dot_git = project_root.join(".git");
    let metadata = std::fs::metadata(&dot_git).map_err(|_| invalid())?;
    if metadata.is_dir() {
        return Ok(dot_git);
    }
    if !metadata.is_file() {
        return Err(invalid());
    }
    let line = read_line(&dot_git, MAX_METADATA_BYTES)?;
    let target = line.strip_prefix(GIT_DIR_PREFIX).ok_or_else(invalid)?;
    Ok(relative_to(project_root, target))
}

fn common_dir(git_dir: &Path) -> Result<PathBuf, DispatchError> {
    match read_optional_line(&git_dir.join("commondir"), MAX_METADATA_BYTES)? {
        Some(target) => Ok(relative_to(git_dir, &target)),
        None => Ok(git_dir.to_path_buf()),
    }
}

/// A linked worktree keeps its own `HEAD` but shares branch refs through the
/// common directory; `packed-refs` lives only there.
fn resolve_ref(git_dir: &Path, common_dir: &Path, name: &str) -> Result<String, DispatchError> {
    for directory in [git_dir, common_dir] {
        if let Some(target) = read_optional_line(&directory.join(name), MAX_METADATA_BYTES)? {
            return Ok(target);
        }
    }
    let packed = read_optional(&common_dir.join("packed-refs"), MAX_PACKED_REFS_BYTES)?
        .ok_or_else(invalid)?;
    packed
        .lines()
        .filter(|line| !line.starts_with('#') && !line.starts_with('^'))
        .find_map(|line| {
            let (object, reference) = line.trim_end_matches('\r').split_once(' ')?;
            (reference == name).then(|| object.to_string())
        })
        .ok_or_else(invalid)
}

fn ref_name(name: &str) -> Result<&str, DispatchError> {
    let well_formed = name.starts_with("refs/")
        && name.len() <= MAX_REF_NAME_BYTES
        && !name.ends_with('/')
        && !name.ends_with(".lock")
        && name
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.'))
        && name.bytes().all(|byte| {
            byte.is_ascii_graphic()
                && !matches!(byte, b'\\' | b':' | b'~' | b'^' | b'?' | b'*' | b'[')
        })
        && !name.contains("..");
    if well_formed {
        Ok(name)
    } else {
        Err(invalid())
    }
}

/// SHA-1 (40) or SHA-256 (64) object id.
fn object_id(text: &str) -> Result<String, DispatchError> {
    if matches!(text.len(), 40 | 64) && text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(text.to_ascii_lowercase())
    } else {
        Err(invalid())
    }
}

fn relative_to(base: &Path, target: &str) -> PathBuf {
    let target = Path::new(target);
    if target.is_absolute() {
        target.to_path_buf()
    } else {
        base.join(target)
    }
}

fn read_line(path: &Path, limit: u64) -> Result<String, DispatchError> {
    read_optional_line(path, limit)?.ok_or_else(invalid)
}

/// The file's first line without its terminator; `None` when it is absent.
fn read_optional_line(path: &Path, limit: u64) -> Result<Option<String>, DispatchError> {
    let Some(text) = read_optional(path, limit)? else {
        return Ok(None);
    };
    let line = text
        .lines()
        .next()
        .unwrap_or_default()
        .trim_end_matches('\r');
    if line.is_empty() {
        return Err(invalid());
    }
    Ok(Some(line.to_string()))
}

fn read_optional(path: &Path, limit: u64) -> Result<Option<String>, DispatchError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(invalid()),
    };
    if !file.metadata().map_err(|_| invalid())?.is_file() {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid())?;
    if bytes.len() as u64 > limit {
        return Err(invalid());
    }
    String::from_utf8(bytes).map(Some).map_err(|_| invalid())
}

const fn invalid() -> DispatchError {
    DispatchError::WorkingDirectoryInvalid
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
    const OTHER: &str = "89abcdef0123456789abcdef0123456789abcdef";

    fn write(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(path, content).expect("write metadata");
    }

    #[test]
    fn a_branch_resolves_through_a_loose_ref_before_packed_refs() {
        let root = tempfile::tempdir().expect("root");
        let git = root.path().join(".git");
        write(&git.join("HEAD"), "ref: refs/heads/main\n");
        write(
            &git.join("packed-refs"),
            &format!("# pack-refs with: peeled\n{OTHER} refs/heads/main\n"),
        );
        assert_eq!(read_head_commit(root.path()).as_deref(), Ok(OTHER));
        write(&git.join("refs/heads/main"), &format!("{COMMIT}\r\n"));
        assert_eq!(read_head_commit(root.path()).as_deref(), Ok(COMMIT));
    }

    #[test]
    fn packed_refs_skip_comments_and_peeled_lines() {
        let root = tempfile::tempdir().expect("root");
        let git = root.path().join(".git");
        write(&git.join("HEAD"), "ref: refs/heads/topic\n");
        write(
            &git.join("packed-refs"),
            &format!("# comment\n{OTHER} refs/heads/main\n^{OTHER}\n{COMMIT} refs/heads/topic\n"),
        );
        assert_eq!(read_head_commit(root.path()).as_deref(), Ok(COMMIT));
    }

    #[test]
    fn a_detached_head_is_its_own_commit_in_lowercase() {
        let root = tempfile::tempdir().expect("root");
        write(&root.path().join(".git/HEAD"), &COMMIT.to_ascii_uppercase());
        assert_eq!(read_head_commit(root.path()).as_deref(), Ok(COMMIT));
    }

    #[test]
    fn a_linked_worktree_reads_branch_refs_from_the_common_directory() {
        let root = tempfile::tempdir().expect("root");
        let worktree = root.path().join("worktree");
        let common = root.path().join("main/.git");
        let private = common.join("worktrees/linked");
        write(
            &worktree.join(".git"),
            "gitdir: ../main/.git/worktrees/linked\n",
        );
        write(&private.join("HEAD"), "ref: refs/heads/feature\n");
        write(&private.join("commondir"), "../..\n");
        write(&common.join("refs/heads/feature"), COMMIT);
        assert_eq!(read_head_commit(&worktree).as_deref(), Ok(COMMIT));
    }

    #[test]
    fn unresolvable_or_malformed_metadata_fails_closed() {
        let cases: [(&str, &str); 6] = [
            ("HEAD", "ref: refs/heads/unborn\n"),
            ("HEAD", "ref: refs/heads/../../escape\n"),
            ("HEAD", "ref: HEAD\n"),
            ("HEAD", "ref: refs/heads/.hidden\n"),
            ("HEAD", "0123\n"),
            ("HEAD", "\n"),
        ];
        for (name, content) in cases {
            let root = tempfile::tempdir().expect("root");
            write(&root.path().join(".git").join(name), content);
            assert_eq!(
                read_head_commit(root.path()),
                Err(DispatchError::WorkingDirectoryInvalid),
                "{content:?}"
            );
        }
        let missing = tempfile::tempdir().expect("root");
        assert_eq!(
            read_head_commit(missing.path()),
            Err(DispatchError::WorkingDirectoryInvalid)
        );
        let oversized = tempfile::tempdir().expect("root");
        write(
            &oversized.path().join(".git/HEAD"),
            &"x".repeat(MAX_METADATA_BYTES as usize + 1),
        );
        assert_eq!(
            read_head_commit(oversized.path()),
            Err(DispatchError::WorkingDirectoryInvalid)
        );
    }

    #[test]
    fn a_symbolic_ref_cycle_stops_at_the_depth_bound() {
        let root = tempfile::tempdir().expect("root");
        let git = root.path().join(".git");
        write(&git.join("HEAD"), "ref: refs/heads/a\n");
        write(&git.join("refs/heads/a"), "ref: refs/heads/b\n");
        write(&git.join("refs/heads/b"), "ref: refs/heads/a\n");
        assert_eq!(
            read_head_commit(root.path()),
            Err(DispatchError::WorkingDirectoryInvalid)
        );
    }
}
