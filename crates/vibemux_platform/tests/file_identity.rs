#![cfg(windows)]
#![forbid(unsafe_code)]
use std::{
    fs::{File, OpenOptions},
    os::windows::fs::OpenOptionsExt,
};
use vibemux_platform::query_windows_file_identity;

fn open_identity(path: &std::path::Path) -> File {
    OpenOptions::new()
        .read(true)
        .share_mode(7)
        .custom_flags(0x0200_0000 | 0x0020_0000)
        .open(path)
        .expect("owned observation handle")
}

#[test]
fn one_open_object_keeps_its_identity_through_alias_and_rename() {
    let root = tempfile::tempdir().expect("owned root");
    let original = root.path().join("original");
    let alias = root.path().join("alias");
    let renamed = root.path().join("renamed");
    std::fs::write(&original, "synthetic").expect("file");
    let handle = open_identity(&original);
    let identity = query_windows_file_identity(&handle).expect("full identity");
    std::fs::hard_link(&original, &alias).expect("same-volume alias");
    assert_eq!(
        query_windows_file_identity(&open_identity(&alias)).expect("alias"),
        identity
    );
    std::fs::rename(&original, &renamed)
        .expect("share-delete observation does not freeze namespace");
    assert_eq!(
        query_windows_file_identity(&handle).expect("same held object"),
        identity
    );
    assert_eq!(
        query_windows_file_identity(&open_identity(&renamed)).expect("renamed"),
        identity
    );
    std::fs::write(&original, "new object").expect("replacement path");
    assert_ne!(
        query_windows_file_identity(&open_identity(&original)).expect("new identity"),
        identity
    );
    assert_eq!(format!("{identity:?}"), "WindowsFileIdentity(<redacted>)");
}

#[test]
fn directory_handles_are_observed_without_path_lookup_or_mutation() {
    let root = tempfile::tempdir().expect("owned root");
    let handle = open_identity(root.path());
    let first = query_windows_file_identity(&handle).expect("directory identity");
    let second = query_windows_file_identity(&handle).expect("same directory");
    assert_eq!(first, second);
    assert!(first.file_id.iter().any(|byte| *byte != 0));
    assert_eq!(
        std::fs::read_dir(root.path())
            .expect("unchanged root")
            .count(),
        0
    );
}

#[test]
fn unsupported_handle_never_becomes_a_zero_filled_success() {
    let device = File::open("NUL").expect("local null device");
    let error = query_windows_file_identity(&device).expect_err("no file identity");
    assert!(matches!(
        error.code(),
        "platform_file_identity_unsupported" | "platform_file_identity_failed"
    ));
    assert!(!error.to_string().contains("NUL"));
}
