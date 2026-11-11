//! Atomic publication shared by the native configuration files.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

pub(crate) fn replace_config_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut name = std::ffi::OsString::from(".");
    name.push(path.file_name().unwrap_or_default());
    name.push(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let temporary = path.with_file_name(name);
    write_new(&temporary, bytes)?;
    fs::rename(&temporary, path).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })
}

pub(crate) fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    written.inspect_err(|_| {
        let _ = fs::remove_file(path);
    })
}
