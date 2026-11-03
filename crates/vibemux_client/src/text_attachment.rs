//! Explicit native selection of bounded UTF-8 file snapshots for a chat draft.
use serde::Serialize;
use std::{
    fmt,
    fs::{File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
};
use uuid::Uuid;

pub const MAX_TEXT_ATTACHMENTS: usize = 4;
pub const MAX_ATTACHMENT_BYTES: u64 = 16 * 1024;
pub const MAX_ATTACHMENT_TOTAL_BYTES: u64 = 24 * 1024;
pub const MAX_ATTACHMENT_NAME_BYTES: usize = 240;
const TEXT_EXTENSIONS: &[&str] = &[
    "txt", "md", "markdown", "rs", "py", "js", "ts", "tsx", "jsx", "json", "toml", "yaml", "yml",
    "csv", "log", "html", "css", "sql", "sh", "ps1", "c", "h", "cpp", "hpp", "java", "go", "rb",
    "xml", "svg",
];
#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
#[cfg(windows)]
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
#[cfg(windows)]
const FILE_SHARE_READ_WRITE: u32 = 0x3;

#[derive(Clone, Serialize, PartialEq, Eq)]
pub struct TextAttachment {
    pub attachment_id: Uuid,
    pub name: String,
    pub content: String,
    pub byte_count: u64,
}
impl fmt::Debug for TextAttachment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextAttachment")
            .field("attachment_id", &self.attachment_id)
            .field("byte_count", &self.byte_count)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachmentError {
    Unsupported,
    Count,
    TooLarge,
    InvalidFile,
    InvalidText,
    InvalidName,
    ReadFailed,
}
impl AttachmentError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Unsupported => "attachment_picker_unsupported",
            Self::Count => "attachment_count_limit",
            Self::TooLarge => "attachment_size_limit",
            Self::InvalidFile => "attachment_invalid_file",
            Self::InvalidText => "attachment_invalid_text",
            Self::InvalidName => "attachment_invalid_name",
            Self::ReadFailed => "attachment_read_failed",
        }
    }
    pub const fn message(self) -> &'static str {
        match self {
            Self::Unsupported => "The native file picker is unavailable on this platform.",
            Self::Count => "Select up to four text or code files.",
            Self::TooLarge => {
                "Each file must fit within 16 KiB, with at most 24 KiB selected together."
            }
            Self::InvalidFile => {
                "Select ordinary text or code files. Linked files and directories are unsupported."
            }
            Self::InvalidText => "An attachment must contain UTF-8 text without NUL bytes.",
            Self::InvalidName => "An attachment filename is invalid or too long.",
            Self::ReadFailed => {
                "The selected file could not be read. Select it again after checking access."
            }
        }
    }
}

/// Paths come only from the native dialog or trusted Rust callers, never JS.
pub fn load_text_attachments(paths: &[PathBuf]) -> Result<Vec<TextAttachment>, AttachmentError> {
    if paths.is_empty() || paths.len() > MAX_TEXT_ATTACHMENTS {
        return Err(AttachmentError::Count);
    }
    let mut result = Vec::with_capacity(paths.len());
    let mut total = 0_u64;
    for path in paths {
        #[cfg(windows)]
        let _ancestors = hold_plain_ancestors(path)?;
        let metadata = std::fs::symlink_metadata(path).map_err(|_| AttachmentError::ReadFailed)?;
        if !metadata.is_file() || is_link(&metadata) {
            return Err(AttachmentError::InvalidFile);
        }
        if metadata.len() > MAX_ATTACHMENT_BYTES {
            return Err(AttachmentError::TooLarge);
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| {
                !name.is_empty()
                    && name.len() <= MAX_ATTACHMENT_NAME_BYTES
                    && !name.chars().any(char::is_control)
            })
            .ok_or(AttachmentError::InvalidName)?
            .to_owned();
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        let file = options
            .open(path)
            .map_err(|_| AttachmentError::ReadFailed)?;
        let content = read_snapshot(file)?;
        let byte_count = content.len() as u64;
        total = total
            .checked_add(byte_count)
            .ok_or(AttachmentError::TooLarge)?;
        if total > MAX_ATTACHMENT_TOTAL_BYTES {
            return Err(AttachmentError::TooLarge);
        }
        result.push(TextAttachment {
            attachment_id: Uuid::new_v4(),
            name,
            content,
            byte_count,
        });
    }
    Ok(result)
}

/// Hold each directory from the root down without delete sharing to prevent
/// ancestor rename/replacement until the selected leaf has been read.
#[cfg(windows)]
fn hold_plain_ancestors(path: &Path) -> Result<Vec<File>, AttachmentError> {
    use std::os::windows::fs::OpenOptionsExt;
    if !path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return Err(AttachmentError::InvalidFile);
    }
    let mut ancestors = path
        .parent()
        .ok_or(AttachmentError::InvalidFile)?
        .ancestors()
        .collect::<Vec<_>>();
    ancestors.reverse();
    let mut held = Vec::with_capacity(ancestors.len());
    for ancestor in ancestors {
        let directory = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(ancestor)
            .map_err(|_| AttachmentError::ReadFailed)?;
        let metadata = directory
            .metadata()
            .map_err(|_| AttachmentError::ReadFailed)?;
        if !metadata.is_dir() || is_link(&metadata) {
            return Err(AttachmentError::InvalidFile);
        }
        held.push(directory);
    }
    Ok(held)
}

fn read_snapshot(file: File) -> Result<String, AttachmentError> {
    let metadata = file.metadata().map_err(|_| AttachmentError::ReadFailed)?;
    if !metadata.is_file() || is_link(&metadata) {
        return Err(AttachmentError::InvalidFile);
    }
    if metadata.len() > MAX_ATTACHMENT_BYTES {
        return Err(AttachmentError::TooLarge);
    }
    let mut bytes = Vec::new();
    file.take(MAX_ATTACHMENT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| AttachmentError::ReadFailed)?;
    if bytes.len() as u64 > MAX_ATTACHMENT_BYTES {
        return Err(AttachmentError::TooLarge);
    }
    if bytes.contains(&0) {
        return Err(AttachmentError::InvalidText);
    }
    let content = String::from_utf8(bytes).map_err(|_| AttachmentError::InvalidText)?;
    Ok(content
        .strip_prefix('\u{feff}')
        .unwrap_or(&content)
        .to_owned())
}

fn is_link(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

pub fn pick_text_attachments(
    start: Option<&Path>,
) -> Result<Option<Vec<TextAttachment>>, AttachmentError> {
    #[cfg(all(windows, feature = "folder_dialog"))]
    {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Attach text or code files")
            .add_filter("Text and code", TEXT_EXTENSIONS);
        if let Some(start) = start.filter(|path| path.is_dir()) {
            dialog = dialog.set_directory(start);
        }
        dialog
            .pick_files()
            .map(|paths| load_text_attachments(&paths))
            .transpose()
    }
    #[cfg(not(all(windows, feature = "folder_dialog")))]
    {
        let _ = (start, TEXT_EXTENSIONS);
        Err(AttachmentError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn junction_ancestors_are_refused() {
        use std::os::windows::process::CommandExt;
        let temp = tempfile::tempdir().expect("fixture");
        let target = temp.path().join("target");
        let link = temp.path().join("linked");
        std::fs::create_dir(&target).expect("target");
        std::fs::write(target.join("body.txt"), "private target").expect("body");
        let powershell = PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"))
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let status = std::process::Command::new(powershell)
            .args(["-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:VIBEMUX_TEST_LINK -Target $env:VIBEMUX_TEST_TARGET | Out-Null"])
            .env("VIBEMUX_TEST_LINK", &link).env("VIBEMUX_TEST_TARGET", &target)
            .creation_flags(0x0800_0000).status().expect("junction fixture");
        assert!(status.success());
        let result = load_text_attachments(&[link.join("body.txt")]);
        std::fs::remove_dir(&link).expect("remove only the owned junction");
        assert_eq!(result, Err(AttachmentError::InvalidFile));
    }
    #[test]
    fn selected_content_is_an_immutable_private_snapshot_without_paths() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("example 中文.txt");
        std::fs::write(&path, "\u{feff}PRIVATE_FILE_BODY_中文").expect("write");
        let files = load_text_attachments(std::slice::from_ref(&path)).expect("read");
        std::fs::write(&path, "later edit").expect("change source");
        assert_eq!(files[0].content, "PRIVATE_FILE_BODY_中文");
        assert_eq!(files[0].byte_count, files[0].content.len() as u64);
        assert!(!format!("{:?}", files[0]).contains("PRIVATE_FILE"));
        let encoded = serde_json::to_string(&files[0]).expect("DTO");
        assert!(!encoded.contains(&temp.path().display().to_string()));
        assert!(encoded.contains("example 中文.txt"));
    }
    #[test]
    fn count_file_and_aggregate_limits_refuse_complete_selection() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("large.txt");
        assert_eq!(load_text_attachments(&[]), Err(AttachmentError::Count));
        assert_eq!(
            load_text_attachments(&vec![path.clone(); 5]),
            Err(AttachmentError::Count)
        );
        std::fs::write(&path, vec![b'a'; MAX_ATTACHMENT_BYTES as usize + 1]).expect("large");
        assert_eq!(
            load_text_attachments(std::slice::from_ref(&path)),
            Err(AttachmentError::TooLarge)
        );
        std::fs::write(&path, vec![b'a'; MAX_ATTACHMENT_BYTES as usize]).expect("bounded");
        assert!(load_text_attachments(std::slice::from_ref(&path)).is_ok());
        assert_eq!(
            load_text_attachments(&[path.clone(), path]),
            Err(AttachmentError::TooLarge)
        );
    }
    #[test]
    fn invalid_text_missing_files_and_directories_are_refused() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("bad.bin");
        assert_eq!(
            load_text_attachments(std::slice::from_ref(&path)),
            Err(AttachmentError::ReadFailed)
        );
        assert_eq!(
            load_text_attachments(&[temp.path().to_owned()]),
            Err(AttachmentError::InvalidFile)
        );
        for bytes in [vec![0xff, 0xfe], vec![b'a', 0, b'b']] {
            std::fs::write(&path, bytes).expect("invalid body");
            assert_eq!(
                load_text_attachments(std::slice::from_ref(&path)),
                Err(AttachmentError::InvalidText)
            );
        }
    }
}
