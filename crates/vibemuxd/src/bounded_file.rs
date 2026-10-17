//! Bounded reads of regular files: the daemon's lifecycle lock, shared
//! contract text from the project checkout, files a worker changed in its
//! worktree, a verifier's result file, and the daemon's own workflow state.
//! The bound holds for the bytes actually read, not only for the size
//! reported before the read.

use std::{
    fs::File,
    io::{ErrorKind, Read},
    path::Path,
};

/// Why a bounded read returned no bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BoundedReadError {
    /// Nothing exists at the path.
    Missing,
    /// A symlink, directory, or other non-regular file, or more bytes than
    /// the bound.
    Refused,
    /// The file could not be inspected, opened, or read.
    Io,
}

/// The bytes of the regular file at `path`, at most `maximum` of them.
///
/// The path is checked without following a symlink, so a link, FIFO, or
/// device found there is never opened. The open itself still follows a link
/// swapped in after that check; the handle check and the bounded read keep
/// whatever was opened to a regular file within the bound, and a source
/// grown past the bound is refused rather than read whole.
pub(crate) fn read_regular_file(path: &Path, maximum: u64) -> Result<Vec<u8>, BoundedReadError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| match error.kind() {
        ErrorKind::NotFound => BoundedReadError::Missing,
        _ => BoundedReadError::Io,
    })?;
    if !metadata.is_file() || metadata.len() > maximum {
        return Err(BoundedReadError::Refused);
    }
    let file = File::open(path).map_err(|_| BoundedReadError::Io)?;
    let opened = file.metadata().map_err(|_| BoundedReadError::Io)?;
    if !opened.is_file() || opened.len() > maximum {
        return Err(BoundedReadError::Refused);
    }
    read_at_most(file, maximum)
}

/// At most `maximum` bytes; a longer source is refused, not truncated,
/// after reading one byte past the bound.
fn read_at_most(reader: impl Read, maximum: u64) -> Result<Vec<u8>, BoundedReadError> {
    let mut bytes = Vec::new();
    reader
        .take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| BoundedReadError::Io)?;
    if bytes.len() as u64 > maximum {
        return Err(BoundedReadError::Refused);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_stops_one_byte_past_the_bound() {
        assert_eq!(read_at_most(&b"12345678"[..], 8), Ok(b"12345678".to_vec()));
        // An endless source ends at the bound instead of exhausting memory.
        assert_eq!(
            read_at_most(std::io::repeat(b'x'), 8),
            Err(BoundedReadError::Refused)
        );
    }

    #[test]
    fn only_a_regular_file_within_the_bound_is_read() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let path = temporary.path().join("result.json");
        std::fs::write(&path, b"12345678").expect("write fixture");
        assert_eq!(read_regular_file(&path, 8), Ok(b"12345678".to_vec()));
        assert_eq!(read_regular_file(&path, 7), Err(BoundedReadError::Refused));
        assert_eq!(
            read_regular_file(temporary.path(), 8),
            Err(BoundedReadError::Refused)
        );
        assert_eq!(
            read_regular_file(&temporary.path().join("missing.json"), 8),
            Err(BoundedReadError::Missing)
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_is_refused_without_being_followed() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let target = temporary.path().join("target.json");
        std::fs::write(&target, b"{}").expect("write target");
        let link = temporary.path().join("link.json");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        assert_eq!(read_regular_file(&link, 8), Err(BoundedReadError::Refused));
    }
}
