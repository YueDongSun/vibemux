#![forbid(unsafe_code)]

#[path = "../icon.rs"]
mod icon;

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().expect("u16 field"))
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("u32 field"))
}

#[test]
fn generated_ico_has_a_complete_uncompressed_bitmap_header() {
    let directory = tempfile::tempdir().expect("icon directory");
    icon::write_icons(directory.path()).expect("generated icons");
    let ico = std::fs::read(directory.path().join("icon.ico")).expect("ICO");
    assert_eq!(u16_at(&ico, 0), 0);
    assert_eq!(u16_at(&ico, 2), 1);
    assert_eq!(u16_at(&ico, 4), 1);
    assert_eq!(ico[6..10], [64, 64, 0, 0]);
    let offset = u32_at(&ico, 18) as usize;
    assert_eq!(offset, 22);
    assert_eq!(u32_at(&ico, offset), 40);
    assert_eq!(u32_at(&ico, offset + 4), 64);
    assert_eq!(u32_at(&ico, offset + 8), 128);
    assert_eq!(u16_at(&ico, offset + 12), 1);
    assert_eq!(u16_at(&ico, offset + 14), 32);
    assert_eq!(u32_at(&ico, offset + 16), 0, "BI_RGB compression field");
    let pixels_and_mask = 64 * 64 * 4 + 64 * 8;
    assert_eq!(u32_at(&ico, offset + 20) as usize, pixels_and_mask);
    assert_eq!(u32_at(&ico, 14) as usize, 40 + pixels_and_mask);
    assert_eq!(ico.len(), offset + 40 + pixels_and_mask);
    let png = std::fs::read(directory.path().join("icon.png")).expect("PNG");
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
}

fn legacy_icon(correct: &[u8]) -> Vec<u8> {
    let mut legacy = correct.to_vec();
    legacy.drain(38..42);
    let payload_size = (legacy.len() - 22) as u32;
    legacy[14..18].copy_from_slice(&payload_size.to_le_bytes());
    legacy
}

#[test]
fn the_exact_old_generated_icon_is_repaired_without_replacing_png() {
    let directory = tempfile::tempdir().expect("icon directory");
    icon::write_icons(directory.path()).expect("generated icons");
    let path = directory.path().join("icon.ico");
    let correct = std::fs::read(&path).expect("ICO");
    std::fs::write(&path, legacy_icon(&correct)).expect("old generated ICO");
    let png = directory.path().join("icon.png");
    std::fs::write(&png, b"operator-supplied PNG sentinel").expect("custom PNG");
    assert!(icon::repair_legacy_icon(directory.path()).expect("repair"));
    assert_eq!(std::fs::read(&path).expect("repaired ICO"), correct);
    assert_eq!(
        std::fs::read(&png).expect("preserved PNG"),
        b"operator-supplied PNG sentinel"
    );
    assert!(!icon::repair_legacy_icon(directory.path()).expect("idempotent"));
}

#[test]
fn a_different_icon_is_not_rewritten_even_with_the_same_legacy_header() {
    let directory = tempfile::tempdir().expect("icon directory");
    icon::write_icons(directory.path()).expect("generated icons");
    let path = directory.path().join("icon.ico");
    let mut custom = legacy_icon(&std::fs::read(&path).expect("ICO"));
    *custom.last_mut().expect("mask byte") ^= 1;
    std::fs::write(&path, &custom).expect("custom ICO");
    assert!(!icon::repair_legacy_icon(directory.path()).expect("preserve custom icon"));
    assert_eq!(std::fs::read(&path).expect("unchanged"), custom);
}

#[cfg(unix)]
#[test]
fn a_link_to_an_old_icon_is_not_rewritten() {
    let directory = tempfile::tempdir().expect("icon directory");
    let generated = directory.path().join("generated");
    icon::write_icons(&generated).expect("generated icons");
    let path = generated.join("icon.ico");
    let legacy = legacy_icon(&std::fs::read(&path).expect("ICO"));
    std::fs::write(&path, &legacy).expect("old ICO");
    let linked = directory.path().join("linked");
    std::fs::create_dir(&linked).expect("linked directory");
    std::os::unix::fs::symlink(&path, linked.join("icon.ico")).expect("link");
    assert!(!icon::repair_legacy_icon(&linked).expect("preserve link"));
    assert_eq!(std::fs::read(&path).expect("unchanged target"), legacy);
}
