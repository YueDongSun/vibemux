#![forbid(unsafe_code)]
//! Startup error log for the console-less GUI.
//!
//! The Windows GUI binary has no console, so a startup failure or a panic
//! would otherwise vanish. Both are appended to `frontend_errors.log` next
//! to the persisted user settings (`theme::serialize::config_path`). The
//! file is bounded: when it would grow past [`MAX_LOG_BYTES`], it is
//! started afresh with only the new entry.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Once,
};

/// File name of the log, a sibling of `frontend.json`.
pub const LOG_FILE_NAME: &str = "frontend_errors.log";
/// The log never grows past this; older entries are dropped.
pub const MAX_LOG_BYTES: u64 = 64 * 1024;
/// One entry never takes more than this many characters of its message.
const MAX_MESSAGE_CHARS: usize = 4096;

/// Where startup errors are written.
#[must_use]
pub fn log_path() -> PathBuf {
    crate::theme::serialize::config_path().with_file_name(LOG_FILE_NAME)
}

/// One log line: a UTC timestamp, then the message on a single line with
/// control characters replaced, cut to [`MAX_MESSAGE_CHARS`].
#[must_use]
pub fn format_entry(timestamp: &str, message: &str) -> String {
    let single_line: String = message
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(MAX_MESSAGE_CHARS)
        .collect();
    format!("{timestamp} {}\n", single_line.trim_end())
}

/// Appends one entry to the log at `path`, creating its directory. A file
/// that the entry would push past [`MAX_LOG_BYTES`] is replaced by the
/// entry alone.
pub fn append_entry(path: &Path, message: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let entry = format_entry(&utc_timestamp(), message);
    let current = fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let restart = current.saturating_add(entry.len() as u64) > MAX_LOG_BYTES;
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .append(!restart)
        .truncate(restart)
        .open(path)?;
    file.write_all(entry.as_bytes())
}

/// Records a startup failure. Best effort: a log that cannot be written
/// must not hide the original error, so the write result is ignored.
pub fn record_startup_error(message: &str) {
    let _ = append_entry(&log_path(), &format!("startup failed: {message}"));
}

/// Writes every panic to the log before the default hook runs. Installed
/// once per process.
pub fn install_panic_hook() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = append_entry(&log_path(), &format!("panic: {info}"));
            previous(info);
        }));
    });
}

fn utc_timestamp() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "unknown_time".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_single_line_and_bounded() {
        let entry = format_entry("2026-10-22T00:00:00Z", "first\nsecond\r\tthird  ");
        assert_eq!(entry, "2026-10-22T00:00:00Z first second  third\n");
        let long = "x".repeat(MAX_MESSAGE_CHARS + 50);
        let entry = format_entry("t", &long);
        assert_eq!(entry.len(), "t ".len() + MAX_MESSAGE_CHARS + 1);
    }

    #[test]
    fn appending_creates_the_directory_and_restarts_a_full_log() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("nested").join(LOG_FILE_NAME);
        append_entry(&path, "one").expect("first entry");
        append_entry(&path, "two").expect("second entry");
        let text = fs::read_to_string(&path).expect("log text");
        assert_eq!(text.lines().count(), 2);
        assert!(
            text.lines()
                .next()
                .is_some_and(|line| line.ends_with(" one"))
        );

        fs::write(&path, "y".repeat(MAX_LOG_BYTES as usize)).expect("fill the log");
        append_entry(&path, "fresh").expect("entry after the bound");
        let text = fs::read_to_string(&path).expect("log text");
        assert_eq!(text.lines().count(), 1);
        assert!(text.ends_with(" fresh\n"));
        assert!(fs::metadata(&path).expect("metadata").len() <= MAX_LOG_BYTES);
    }

    #[test]
    fn the_log_sits_next_to_the_settings_file() {
        let settings = crate::theme::serialize::config_path();
        assert_eq!(log_path().parent(), settings.parent());
        assert_eq!(
            log_path().file_name().and_then(|name| name.to_str()),
            Some(LOG_FILE_NAME)
        );
    }
}
