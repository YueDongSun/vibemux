//! The prompt a command sends to the daemon. It comes from a `--prompt-file`
//! path or stdin, never from argv, so it stays out of process listings and
//! shell history. One reader bounds it before any byte reaches the daemon;
//! each command keeps its own bound and maps a failure to its own error code.

use std::{ffi::OsString, io::Read, path::PathBuf};

use crate::DaemonCliError;

/// `--prompt-file` value that reads the prompt from stdin.
const STDIN_PROMPT_SOURCE: &str = "-";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PromptSource {
    Stdin,
    File(PathBuf),
}

/// A `--prompt-file` value: a path, or `-` for stdin.
pub(crate) fn prompt_source(value: Option<&OsString>) -> Result<PromptSource, DaemonCliError> {
    let value = value.ok_or(DaemonCliError::InvalidArguments)?;
    Ok(if value.as_os_str() == STDIN_PROMPT_SOURCE {
        PromptSource::Stdin
    } else {
        PromptSource::File(PathBuf::from(value))
    })
}

/// Why a prompt was not read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PromptReadError {
    /// The source could not be opened or read.
    Unavailable,
    /// Over the bound, not UTF-8, or blank.
    Invalid,
}

impl PromptReadError {
    /// `cli_prompt_unavailable` for every command, and the command's own
    /// code for an invalid prompt.
    pub(crate) fn into_cli_error(self, invalid: DaemonCliError) -> DaemonCliError {
        match self {
            Self::Unavailable => DaemonCliError::PromptUnavailable,
            Self::Invalid => invalid,
        }
    }
}

/// Reads the prompt on a blocking thread, off the async runtime.
pub(crate) async fn read_prompt(
    source: PromptSource,
    max_bytes: usize,
) -> Result<String, PromptReadError> {
    tokio::task::spawn_blocking(move || read_prompt_blocking(&source, max_bytes))
        .await
        .map_err(|_| PromptReadError::Unavailable)?
}

/// Reads at most one byte beyond `max_bytes`, so an oversized source is
/// refused without being read whole.
fn read_prompt_blocking(
    source: &PromptSource,
    max_bytes: usize,
) -> Result<String, PromptReadError> {
    let limit = u64::try_from(max_bytes)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut bytes = Vec::new();
    let read = match source {
        PromptSource::Stdin => std::io::stdin().lock().take(limit).read_to_end(&mut bytes),
        PromptSource::File(path) => {
            std::fs::File::open(path).and_then(|file| file.take(limit).read_to_end(&mut bytes))
        }
    };
    read.map_err(|_| PromptReadError::Unavailable)?;
    if bytes.len() > max_bytes {
        return Err(PromptReadError::Invalid);
    }
    let prompt = String::from_utf8(bytes).map_err(|_| PromptReadError::Invalid)?;
    if prompt.trim().is_empty() {
        return Err(PromptReadError::Invalid);
    }
    Ok(prompt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prompt_file_argument_is_a_path_or_stdin() {
        let argument = |text: &str| OsString::from(text);
        assert_eq!(prompt_source(Some(&argument("-"))), Ok(PromptSource::Stdin));
        assert_eq!(
            prompt_source(Some(&argument("C:\\prompts\\a b.txt"))),
            Ok(PromptSource::File(PathBuf::from("C:\\prompts\\a b.txt")))
        );
        assert_eq!(prompt_source(None), Err(DaemonCliError::InvalidArguments));
    }

    #[test]
    fn a_prompt_is_bounded_utf8_and_non_blank() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("prompt.txt");
        let read = |content: &[u8]| {
            std::fs::write(&path, content).expect("write prompt");
            read_prompt_blocking(&PromptSource::File(path.clone()), 8)
        };
        assert_eq!(read("é text".as_bytes()), Ok("é text".to_string()));
        assert_eq!(read(b"12345678"), Ok("12345678".to_string()));
        for invalid in [&b"123456789"[..], &[0xff, 0xfe], b" \n\t"] {
            assert_eq!(read(invalid), Err(PromptReadError::Invalid), "{invalid:?}");
        }
    }

    #[tokio::test]
    async fn a_missing_file_or_a_directory_is_unavailable() {
        let temp = tempfile::tempdir().expect("temp");
        for unreadable in [temp.path().join("missing.txt"), temp.path().to_path_buf()] {
            assert_eq!(
                read_prompt(PromptSource::File(unreadable), 8).await,
                Err(PromptReadError::Unavailable)
            );
        }
    }

    #[test]
    fn each_command_keeps_its_own_invalid_prompt_code() {
        for invalid in [
            DaemonCliError::InvalidPrompt,
            DaemonCliError::InvalidArguments,
        ] {
            assert_eq!(
                PromptReadError::Unavailable.into_cli_error(invalid.clone()),
                DaemonCliError::PromptUnavailable
            );
            assert_eq!(
                PromptReadError::Invalid.into_cli_error(invalid.clone()),
                invalid
            );
        }
    }
}
