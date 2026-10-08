#![forbid(unsafe_code)]
use std::{env, fs, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("cargo manifest directory"));
    let dist = root.join("web/dist");
    println!("cargo:rerun-if-changed=web/dist");
    for name in ["index.html", "sw.js", "manifest.webmanifest"] {
        assert!(
            fs::metadata(dist.join(name)).is_ok_and(|m| m.is_file() && m.len() > 0),
            "vibemux_web_frontend_missing: run npm ci && npm run build in crates/vibemux_web/web before cargo"
        );
    }
    let entries =
        fs::read_dir(dist.join("assets")).expect("vibemux_web_frontend_empty: missing assets");
    assert!(
        entries.filter_map(Result::ok).any(|entry| entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "js")
            && entry.metadata().is_ok_and(|metadata| metadata.len() > 0)),
        "vibemux_web_frontend_empty: missing javascript bundle"
    );
}
