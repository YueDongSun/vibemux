//! Immutable candidate snapshots and artifact scope checks (ADR 031 §6).
//!
//! The daemon observes a worker's worktree after a quiescent checkpoint
//! (tracked, untracked, and ignored changes, deletions, and file types) and
//! hands the observations here. A manifest is admitted only when every
//! change is a regular text file (or a deletion) inside the task's owned
//! paths and outside its forbidden and protected paths. Protected and
//! forbidden paths compare case-insensitively and owned paths compare
//! exactly, so a Windows case alias always fails closed. Any violation
//! fails the Run; it is not "cleaned up" by dropping the offending file.
//!
//! These checks are cooperative artifact admission, not an OS sandbox: a
//! same-user process can write anywhere it can reach, and only the declared
//! sandbox backend could prevent that.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    PathPattern, Sha256Digest,
    canonical_json::{CanonicalError, canonical_digest},
    identifiers::is_relative_path,
};

pub const SNAPSHOT_SCHEMA_VERSION: u32 = 1;
pub const SNAPSHOT_DIGEST_DOMAIN: &str = "vibemux.workflow.candidate_snapshot.v1";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedFileType {
    Regular,
    Symlink,
    /// Directory junctions, reparse points, devices, sockets, and the like.
    Special,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedChangeKind {
    Added,
    Modified,
    Deleted,
}

/// One change the daemon observed. Content facts are absent for deletions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedChange {
    pub path: String,
    pub kind: ObservedChangeKind,
    pub file_type: ObservedFileType,
    pub mode_changed: bool,
    pub content: Option<ObservedContent>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObservedContent {
    pub sha256: Sha256Digest,
    pub byte_count: u64,
    /// Valid UTF-8 without NUL bytes.
    pub is_text: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum SnapshotChange {
    Write {
        sha256: Sha256Digest,
        byte_count: u64,
    },
    Delete,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotEntry {
    pub path: String,
    pub change: SnapshotChange,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotManifest {
    pub schema_version: u32,
    pub base_commit: String,
    pub entries: Vec<SnapshotEntry>,
}

impl SnapshotManifest {
    pub fn digest(&self) -> Result<Sha256Digest, CanonicalError> {
        canonical_digest(SNAPSHOT_DIGEST_DOMAIN, self)
    }

    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.entries
            .iter()
            .map(|entry| match entry.change {
                SnapshotChange::Write { byte_count, .. } => byte_count,
                SnapshotChange::Delete => 0,
            })
            .sum()
    }

    #[must_use]
    pub fn paths(&self) -> Vec<&str> {
        self.entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ScopePolicy<'a> {
    pub owned: &'a [PathPattern],
    pub forbidden: &'a [PathPattern],
    pub protected: &'a [PathPattern],
    pub max_files: usize,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case", tag = "violation", content = "path")]
pub enum ScopeViolation {
    InvalidPath(String),
    OutsideOwnedPaths(String),
    ProtectedPath(String),
    ForbiddenPath(String),
    Symlink(String),
    SpecialFile(String),
    ModeChange(String),
    BinaryContent(String),
    FileTooLarge(String),
    TooManyFiles,
    TotalTooLarge,
    Empty,
}

impl ScopeViolation {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidPath(_) => "snapshot_invalid_path",
            Self::OutsideOwnedPaths(_) => "snapshot_outside_owned_paths",
            Self::ProtectedPath(_) => "snapshot_protected_path",
            Self::ForbiddenPath(_) => "snapshot_forbidden_path",
            Self::Symlink(_) => "snapshot_symlink",
            Self::SpecialFile(_) => "snapshot_special_file",
            Self::ModeChange(_) => "snapshot_mode_change",
            Self::BinaryContent(_) => "snapshot_binary_content",
            Self::FileTooLarge(_) => "snapshot_file_too_large",
            Self::TooManyFiles => "snapshot_too_many_files",
            Self::TotalTooLarge => "snapshot_total_too_large",
            Self::Empty => "snapshot_empty",
        }
    }

    /// A change to a protected or forbidden path fails the Run outright.
    #[must_use]
    pub const fn fails_run(&self) -> bool {
        matches!(self, Self::ProtectedPath(_) | Self::ForbiddenPath(_))
    }
}

fn matches_case_insensitively(pattern: &PathPattern, path: &str) -> bool {
    let lower_path = path.to_ascii_lowercase();
    let lower_pattern = pattern.as_str().to_ascii_lowercase();
    PathPattern::new(lower_pattern).is_ok_and(|lowered| lowered.matches(&lower_path))
}

/// Builds the manifest, or every violation found.
pub fn build_manifest(
    base_commit: &str,
    changes: &[ObservedChange],
    scope: ScopePolicy<'_>,
) -> Result<SnapshotManifest, Vec<ScopeViolation>> {
    let mut violations = Vec::new();
    if changes.is_empty() {
        violations.push(ScopeViolation::Empty);
    }
    if changes.len() > scope.max_files {
        violations.push(ScopeViolation::TooManyFiles);
    }
    let mut entries = BTreeMap::new();
    let mut total = 0_u64;
    for change in changes {
        let path = change.path.as_str();
        if !is_relative_path(path) {
            violations.push(ScopeViolation::InvalidPath(path.to_string()));
            continue;
        }
        if scope
            .protected
            .iter()
            .any(|pattern| matches_case_insensitively(pattern, path))
        {
            violations.push(ScopeViolation::ProtectedPath(path.to_string()));
        } else if scope
            .forbidden
            .iter()
            .any(|pattern| matches_case_insensitively(pattern, path))
        {
            violations.push(ScopeViolation::ForbiddenPath(path.to_string()));
        } else if !scope.owned.iter().any(|pattern| pattern.matches(path)) {
            violations.push(ScopeViolation::OutsideOwnedPaths(path.to_string()));
        }
        match change.file_type {
            ObservedFileType::Symlink => violations.push(ScopeViolation::Symlink(path.to_string())),
            ObservedFileType::Special => {
                violations.push(ScopeViolation::SpecialFile(path.to_string()))
            }
            ObservedFileType::Regular => {}
        }
        if change.mode_changed {
            violations.push(ScopeViolation::ModeChange(path.to_string()));
        }
        let snapshot_change = match (change.kind, change.content) {
            (ObservedChangeKind::Deleted, _) => SnapshotChange::Delete,
            (_, Some(content)) => {
                if !content.is_text {
                    violations.push(ScopeViolation::BinaryContent(path.to_string()));
                }
                if content.byte_count > scope.max_file_bytes {
                    violations.push(ScopeViolation::FileTooLarge(path.to_string()));
                }
                total = total.saturating_add(content.byte_count);
                SnapshotChange::Write {
                    sha256: content.sha256,
                    byte_count: content.byte_count,
                }
            }
            (_, None) => {
                violations.push(ScopeViolation::InvalidPath(path.to_string()));
                continue;
            }
        };
        entries.insert(path.to_string(), snapshot_change);
    }
    if total > scope.max_total_bytes {
        violations.push(ScopeViolation::TotalTooLarge);
    }
    if !violations.is_empty() {
        violations.sort();
        violations.dedup();
        return Err(violations);
    }
    Ok(SnapshotManifest {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        base_commit: base_commit.to_string(),
        entries: entries
            .into_iter()
            .map(|(path, change)| SnapshotEntry { path, change })
            .collect(),
    })
}

/// Paths whose recorded content differs between two collections of the
/// same candidate (for example before and after review or verification).
#[must_use]
pub fn mutated_paths(collected: &SnapshotManifest, observed: &SnapshotManifest) -> Vec<String> {
    let before: BTreeMap<&str, &SnapshotChange> = collected
        .entries
        .iter()
        .map(|entry| (entry.path.as_str(), &entry.change))
        .collect();
    let after: BTreeMap<&str, &SnapshotChange> = observed
        .entries
        .iter()
        .map(|entry| (entry.path.as_str(), &entry.change))
        .collect();
    let mut changed: Vec<String> = before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .map(|path| (*path).to_string())
        .collect();
    changed.sort();
    changed.dedup();
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(value: &str) -> PathPattern {
        PathPattern::new(value).expect("pattern")
    }

    fn write(path: &str, kind: ObservedChangeKind, bytes: &[u8]) -> ObservedChange {
        ObservedChange {
            path: path.to_string(),
            kind,
            file_type: ObservedFileType::Regular,
            mode_changed: false,
            content: Some(ObservedContent {
                sha256: Sha256Digest::of(bytes),
                byte_count: bytes.len() as u64,
                is_text: std::str::from_utf8(bytes).is_ok() && !bytes.contains(&0),
            }),
        }
    }

    fn scope<'a>(
        owned: &'a [PathPattern],
        forbidden: &'a [PathPattern],
        protected: &'a [PathPattern],
    ) -> ScopePolicy<'a> {
        ScopePolicy {
            owned,
            forbidden,
            protected,
            max_files: 64,
            max_file_bytes: 256 * 1024,
            max_total_bytes: 1024 * 1024,
        }
    }

    #[test]
    fn untracked_files_and_deletions_inside_owned_paths_are_collected() {
        let owned = [
            pattern("src/server.mjs"),
            pattern("src/store.mjs"),
            pattern("tests/worker_a/**"),
        ];
        let protected = [pattern("package.json")];
        let changes = [
            write("src/server.mjs", ObservedChangeKind::Modified, b"server"),
            write(
                "tests/worker_a/new_untracked.test.mjs",
                ObservedChangeKind::Added,
                b"test",
            ),
            ObservedChange {
                path: "src/store.mjs".into(),
                kind: ObservedChangeKind::Deleted,
                file_type: ObservedFileType::Regular,
                mode_changed: false,
                content: None,
            },
        ];
        let manifest =
            build_manifest("abc", &changes, scope(&owned, &[], &protected)).expect("manifest");
        assert_eq!(
            manifest.paths(),
            vec![
                "src/server.mjs",
                "src/store.mjs",
                "tests/worker_a/new_untracked.test.mjs"
            ]
        );
        assert_eq!(manifest.entries[1].change, SnapshotChange::Delete);
        assert_eq!(manifest.total_bytes(), 10);
        let reordered: Vec<ObservedChange> = changes.iter().rev().cloned().collect();
        assert_eq!(
            build_manifest("abc", &reordered, scope(&owned, &[], &protected))
                .expect("manifest")
                .digest(),
            manifest.digest()
        );
    }

    #[test]
    fn out_of_scope_protected_and_case_alias_changes_fail() {
        let owned = [pattern("public/**")];
        let protected = [pattern("package.json"), pattern("verifier/**")];
        let forbidden = [pattern("src/**")];
        let changes = [
            write("public/app.mjs", ObservedChangeKind::Modified, b"ok"),
            write("Package.json", ObservedChangeKind::Modified, b"{}"),
            write(
                "Verifier/api_suite.test.mjs",
                ObservedChangeKind::Modified,
                b"x",
            ),
            write("src/server.mjs", ObservedChangeKind::Modified, b"x"),
            write("README.md", ObservedChangeKind::Added, b"x"),
            write("Public/app.mjs", ObservedChangeKind::Added, b"x"),
        ];
        let violations = build_manifest("abc", &changes, scope(&owned, &forbidden, &protected))
            .expect_err("violations");
        assert!(violations.contains(&ScopeViolation::ProtectedPath("Package.json".into())));
        assert!(violations.contains(&ScopeViolation::ProtectedPath(
            "Verifier/api_suite.test.mjs".into()
        )));
        assert!(violations.contains(&ScopeViolation::ForbiddenPath("src/server.mjs".into())));
        assert!(violations.contains(&ScopeViolation::OutsideOwnedPaths("README.md".into())));
        assert!(violations.contains(&ScopeViolation::OutsideOwnedPaths("Public/app.mjs".into())));
        assert!(violations.iter().any(ScopeViolation::fails_run));
    }

    #[test]
    fn symlinks_special_files_modes_and_binary_content_are_rejected() {
        let owned = [pattern("public/**")];
        let mut symlink = write("public/link.mjs", ObservedChangeKind::Added, b"x");
        symlink.file_type = ObservedFileType::Symlink;
        let mut junction = write("public/dir", ObservedChangeKind::Added, b"x");
        junction.file_type = ObservedFileType::Special;
        let mut executable = write("public/app.mjs", ObservedChangeKind::Modified, b"x");
        executable.mode_changed = true;
        let binary = write(
            "public/blob.bin",
            ObservedChangeKind::Added,
            &[0, 159, 146, 150],
        );
        let codes: Vec<&str> = build_manifest(
            "abc",
            &[symlink, junction, executable, binary],
            scope(&owned, &[], &[]),
        )
        .expect_err("violations")
        .iter()
        .map(ScopeViolation::code)
        .collect();
        for expected in [
            "snapshot_symlink",
            "snapshot_special_file",
            "snapshot_mode_change",
            "snapshot_binary_content",
        ] {
            assert!(codes.contains(&expected), "{expected}");
        }
        assert_eq!(
            build_manifest("abc", &[], scope(&owned, &[], &[])),
            Err(vec![ScopeViolation::Empty])
        );
    }

    #[test]
    fn traversal_paths_are_invalid() {
        let owned = [pattern("src/**")];
        for path in ["src/../package.json", "/etc/passwd", "src\\x.mjs", "C:/x"] {
            let violations = build_manifest(
                "abc",
                &[write(path, ObservedChangeKind::Added, b"x")],
                scope(&owned, &[], &[]),
            )
            .expect_err("invalid");
            assert_eq!(
                violations,
                vec![ScopeViolation::InvalidPath(path.to_string())]
            );
        }
    }

    #[test]
    fn mutation_after_collection_is_detected() {
        let owned = [pattern("src/**")];
        let collected = build_manifest(
            "abc",
            &[write("src/a.mjs", ObservedChangeKind::Added, b"one")],
            scope(&owned, &[], &[]),
        )
        .expect("manifest");
        let same = build_manifest(
            "abc",
            &[write("src/a.mjs", ObservedChangeKind::Added, b"one")],
            scope(&owned, &[], &[]),
        )
        .expect("manifest");
        assert!(mutated_paths(&collected, &same).is_empty());
        let changed = build_manifest(
            "abc",
            &[
                write("src/a.mjs", ObservedChangeKind::Added, b"two"),
                write("src/b.mjs", ObservedChangeKind::Added, b"new"),
            ],
            scope(&owned, &[], &[]),
        )
        .expect("manifest");
        assert_eq!(
            mutated_paths(&collected, &changed),
            vec!["src/a.mjs".to_string(), "src/b.mjs".to_string()]
        );
    }
}
