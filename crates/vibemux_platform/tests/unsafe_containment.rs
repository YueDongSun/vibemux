//! Mechanical enforcement of the workspace `unsafe` containment policy
//! (ADR 020/025): `vibemux_platform` denies unsafe at the crate root and
//! allows it in exactly one reviewed module (`windows_acl_native`), and
//! every other workspace library crate keeps `#![forbid(unsafe_code)]`.
//! Review code alone drifts (code-review V13 finding on issue #8); these
//! tests fail the moment a second module opts into unsafe or a crate root
//! loses its lint.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Recursively collect every `.rs` file under `dir` (skipping nothing -
/// `src/` has no generated subtrees).
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("source directory listing") {
        let entry = entry.expect("source directory entry");
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// The lint directive must be an inner attribute at the very top of the
/// file (an optional shebang aside). Matching only line starts keeps
/// comments and doc text from counting as directives.
fn count_inner_attribute(source: &str, attribute: &str) -> usize {
    source
        .lines()
        .filter(|line| line.trim_start().starts_with(attribute))
        .count()
}

#[test]
fn platform_crate_root_denies_unsafe_code() {
    let lib_rs = std::fs::read_to_string(crate_root().join("src/lib.rs")).expect("read lib.rs");
    assert_eq!(
        count_inner_attribute(&lib_rs, "#![deny(unsafe_code)]"),
        1,
        "vibemux_platform must deny unsafe at the crate root exactly once \
         (ADR 025: the single-module allow in windows_acl_native relies on it)"
    );
}

#[test]
fn unsafe_allow_is_confined_to_the_reviewed_acl_module() {
    let mut files = Vec::new();
    rust_files(&crate_root().join("src"), &mut files);
    assert!(
        files.len() >= 3,
        "expected lib.rs + windows modules, found {files:?}"
    );
    let mut allow_sites = Vec::new();
    for file in &files {
        let source = std::fs::read_to_string(file).expect("read source file");
        let hits = count_inner_attribute(&source, "#![allow(unsafe_code)]");
        if hits > 0 {
            allow_sites.push((file.clone(), hits));
        }
    }
    let reviewed = crate_root().join("src/windows_acl_native.rs");
    assert_eq!(
        allow_sites,
        vec![(reviewed, 1)],
        "`#![allow(unsafe_code)]` must appear exactly once, in the single \
         module ADR 025 reviewed; any other site widens the unsafe surface \
         without an accepted ADR"
    );
}

#[test]
fn every_workspace_library_crate_forbids_unsafe_code() {
    let crates_dir = crate_root()
        .parent()
        .expect("crates directory")
        .to_path_buf();
    let mut checked = 0usize;
    for entry in std::fs::read_dir(&crates_dir).expect("workspace crates listing") {
        let lib_rs = entry.expect("crate entry").path().join("src/lib.rs");
        if !lib_rs.is_file() {
            continue;
        }
        let source = std::fs::read_to_string(&lib_rs).expect("read crate lib.rs");
        let forbids = count_inner_attribute(&source, "#![forbid(unsafe_code)]");
        if lib_rs.parent() == Some(crate_root().join("src").as_path()) {
            // vibemux_platform is covered by the deny + single-allow tests.
            continue;
        }
        assert_eq!(
            forbids,
            1,
            "{} must keep `#![forbid(unsafe_code)]` (AGENTS.md \
             working conventions)",
            lib_rs.display()
        );
        checked += 1;
    }
    assert!(
        checked >= 8,
        "sanity: the workspace has more library crates than {checked}; \
         a listing failure must not pass as compliance"
    );
}
