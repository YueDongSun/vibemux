#![forbid(unsafe_code)]
//! Generated ICO conformance and exact legacy-cache recovery.

#[path = "../icon.rs"]
mod icon;

fn dword(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("DWORD"))
}

#[test]
fn generated_ico_has_a_complete_uncompressed_bitmap_header() {
    let temp = tempfile::tempdir().expect("owned generated assets");
    icon::write_icons(temp.path()).expect("generate icons");
    let bytes = std::fs::read(temp.path().join("icon.ico")).expect("ICO");
    assert_eq!(&bytes[..6], &[0, 0, 1, 0, 1, 0], "one ICO image");
    let width = usize::from(bytes[6]);
    let height = usize::from(bytes[7]);
    let image_bytes = dword(&bytes, 14) as usize;
    let offset = dword(&bytes, 18) as usize;
    assert_eq!(offset + image_bytes, bytes.len());
    assert_eq!(dword(&bytes, offset), 40, "BITMAPINFOHEADER");
    assert_eq!(dword(&bytes, offset + 4) as usize, width);
    assert_eq!(dword(&bytes, offset + 8) as usize, height * 2);
    assert_eq!(&bytes[offset + 12..offset + 16], &[1, 0, 32, 0]);
    assert_eq!(dword(&bytes, offset + 16), 0, "BI_RGB");
    let pixel_bytes = width * height * 4;
    let mask_bytes = width.div_ceil(32) * 4 * height;
    assert_eq!(image_bytes, 40 + pixel_bytes + mask_bytes);
    assert_eq!(
        dword(&bytes, offset + 20) as usize,
        pixel_bytes + mask_bytes
    );
}

#[test]
fn ensure_repairs_the_exact_golden_legacy_ico_and_preserves_existing_png() {
    let temp = tempfile::tempdir().expect("owned legacy assets");
    let legacy = include_bytes!("fixtures/legacy_fallback_icon.ico");
    let custom_png = b"existing custom PNG bytes stay unchanged";
    std::fs::write(temp.path().join("icon.ico"), legacy).expect("legacy ICO");
    std::fs::write(temp.path().join("icon.png"), custom_png).expect("existing PNG");

    icon::write_icons(temp.path()).expect("repair only known legacy");
    let repaired = std::fs::read(temp.path().join("icon.ico")).expect("repaired ICO");
    assert_eq!(repaired.len(), legacy.len() + 4);
    let reference = tempfile::tempdir().expect("owned reference assets");
    icon::write_icons(reference.path()).expect("explicit generator");
    assert_eq!(
        repaired,
        std::fs::read(reference.path().join("icon.ico")).expect("correct generated ICO")
    );
    assert_eq!(
        std::fs::read(temp.path().join("icon.png")).expect("preserved PNG"),
        custom_png
    );

    icon::write_icons(temp.path()).expect("already repaired assets");
    assert_eq!(
        std::fs::read(temp.path().join("icon.ico")).expect("unchanged corrected ICO"),
        repaired
    );
    assert_eq!(
        std::fs::read(temp.path().join("icon.png")).expect("unchanged PNG"),
        custom_png
    );
}

#[test]
fn ensure_preserves_a_legacy_header_lookalike_with_one_changed_pixel() {
    let temp = tempfile::tempdir().expect("owned custom assets");
    let golden = include_bytes!("fixtures/legacy_fallback_icon.ico");
    let mut custom_ico = golden.to_vec();
    let first_pixel = dword(&custom_ico, 18) as usize + 36;
    custom_ico[first_pixel] ^= 1;
    assert_eq!(&custom_ico[..first_pixel], &golden[..first_pixel]);
    assert_eq!(custom_ico.len(), golden.len());
    assert_ne!(&custom_ico[..], &golden[..]);
    let custom_png = b"existing PNG";
    std::fs::write(temp.path().join("icon.ico"), &custom_ico).expect("lookalike ICO");
    std::fs::write(temp.path().join("icon.png"), custom_png).expect("existing PNG");

    icon::write_icons(temp.path()).expect("preserve unknown ICO");
    assert_eq!(
        std::fs::read(temp.path().join("icon.ico")).expect("unchanged lookalike"),
        custom_ico
    );
    assert_eq!(
        std::fs::read(temp.path().join("icon.png")).expect("unchanged PNG"),
        custom_png
    );
}

#[test]
fn ensure_creates_missing_png_without_replacing_a_custom_ico() {
    let temp = tempfile::tempdir().expect("owned missing PNG");
    let custom_ico = b"custom ICO asset is not owned by the fallback generator";
    std::fs::write(temp.path().join("icon.ico"), custom_ico).expect("custom ICO");

    icon::write_icons(temp.path()).expect("create only missing PNG");
    assert_eq!(
        std::fs::read(temp.path().join("icon.ico")).expect("preserved custom ICO"),
        custom_ico
    );
    let png = std::fs::read(temp.path().join("icon.png")).expect("generated PNG");
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
}

#[test]
fn ensure_creates_missing_ico_without_replacing_an_existing_png() {
    let temp = tempfile::tempdir().expect("owned missing ICO");
    let custom_png = b"custom PNG asset is not owned by the fallback generator";
    std::fs::write(temp.path().join("icon.png"), custom_png).expect("custom PNG");

    icon::write_icons(temp.path()).expect("create only missing ICO");
    assert_eq!(
        std::fs::read(temp.path().join("icon.png")).expect("preserved custom PNG"),
        custom_png
    );
    let ico = std::fs::read(temp.path().join("icon.ico")).expect("generated ICO");
    let image_offset = dword(&ico, 18) as usize;
    assert_eq!(dword(&ico, image_offset), 40);
    assert_eq!(dword(&ico, image_offset + 16), 0);
    assert_eq!(image_offset + dword(&ico, 14) as usize, ico.len());
}
