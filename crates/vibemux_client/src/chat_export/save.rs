//! Native-selected private exports use exclusive new files and pinned ancestors.
use super::{ExportError, ExportFormat, MAX_EXPORT_BYTES};
use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::{
    fs::{File, Metadata, OpenOptions},
    io::Write,
    path::{Component, Prefix},
};

#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
#[cfg(windows)]
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
#[cfg(windows)]
const FILE_SHARE_READ: u32 = 0x1;

/// Ask the user for a destination without supplying a repository directory.
pub fn pick_export_destination(format: ExportFormat) -> Result<Option<PathBuf>, ExportError> {
    #[cfg(all(windows, feature = "folder_dialog"))]
    {
        let (name, label) = match format {
            ExportFormat::Json => ("conversation.json", "JSON conversation"),
            ExportFormat::Text => ("conversation.txt", "Text conversation"),
        };
        Ok(rfd::FileDialog::new()
            .set_title("Export private conversation")
            .set_file_name(name)
            .add_filter(label, &[format.extension()])
            .save_file())
    }
    #[cfg(not(all(windows, feature = "folder_dialog")))]
    {
        let _ = format;
        Err(ExportError::PickerUnsupported)
    }
}

/// The path must come from native selection or another trusted Rust caller.
/// Renderer-supplied destinations must never reach this filesystem boundary.
/// A failed write can leave a partial new file; uncertain files are not removed.
pub fn write_export(path: &Path, bytes: &[u8], format: ExportFormat) -> Result<(), ExportError> {
    let byte_count = u64::try_from(bytes.len()).map_err(|_| ExportError::Limit)?;
    if byte_count > MAX_EXPORT_BYTES {
        return Err(ExportError::Limit);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        validate_destination(path, format)?;
        let _ancestors = hold_plain_ancestors(path)?;
        match std::fs::symlink_metadata(path) {
            Ok(metadata) => return Err(existing_destination(&metadata)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(ExportError::WriteFailed),
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .map_err(|error| match std::fs::symlink_metadata(path) {
                Ok(metadata) => existing_destination(&metadata),
                Err(_) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    ExportError::DestinationExists
                }
                Err(_) => ExportError::WriteFailed,
            })?;
        let metadata = file.metadata().map_err(|_| ExportError::WriteFailed)?;
        if !metadata.is_file() || is_link(&metadata) {
            return Err(ExportError::InvalidDestination);
        }
        file.write_all(bytes)
            .map_err(|_| ExportError::WriteFailed)?;
        file.sync_all().map_err(|_| ExportError::WriteFailed)
    }
    #[cfg(not(windows))]
    {
        let _ = (path, format);
        Err(ExportError::PickerUnsupported)
    }
}

#[cfg(windows)]
fn validate_destination(path: &Path, format: ExportFormat) -> Result<(), ExportError> {
    let invalid = ExportError::InvalidDestination;
    let raw_path = path.to_str().ok_or(invalid)?;
    if !path.is_absolute()
        || raw_path.ends_with(['\\', '/'])
        || raw_path
            .split(['\\', '/'])
            .any(|part| matches!(part, "." | ".."))
        || !path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case(format.extension()))
    {
        return Err(invalid);
    }
    let mut components = path.components();
    match components.next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(_) => {}
            Prefix::UNC(server, share) => {
                if !server.to_str().is_some_and(plain_name)
                    || !share.to_str().is_some_and(plain_name)
                {
                    return Err(invalid);
                }
            }
            _ => return Err(invalid),
        },
        _ => return Err(invalid),
    }
    if !matches!(components.next(), Some(Component::RootDir)) {
        return Err(invalid);
    }
    let mut has_leaf = false;
    for component in components {
        let Component::Normal(name) = component else {
            return Err(invalid);
        };
        if !name.to_str().is_some_and(plain_name) {
            return Err(invalid);
        }
        has_leaf = true;
    }
    if !has_leaf {
        return Err(invalid);
    }
    Ok(())
}

#[cfg(windows)]
fn plain_name(name: &str) -> bool {
    if name.is_empty()
        || name.ends_with(['.', ' '])
        || name.chars().any(|character| {
            character.is_control()
                || matches!(
                    character,
                    ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\\' | '/'
                )
        })
    {
        return false;
    }
    let stem = name.split('.').next().unwrap_or("").trim_end_matches(' ');
    if ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"]
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
    {
        return false;
    }
    if let Some(number) = stem.get(3..) {
        let prefix = stem.get(..3).unwrap_or("");
        if (prefix.eq_ignore_ascii_case("COM") || prefix.eq_ignore_ascii_case("LPT"))
            && matches!(
                number,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        {
            return false;
        }
    }
    true
}

/// Keep checked ancestor directories open without write or delete sharing until the
/// selected file has been written and synced.
#[cfg(windows)]
fn hold_plain_ancestors(path: &Path) -> Result<Vec<File>, ExportError> {
    use std::os::windows::fs::OpenOptionsExt;
    let mut ancestors = path
        .parent()
        .ok_or(ExportError::InvalidDestination)?
        .ancestors()
        .collect::<Vec<_>>();
    ancestors.reverse();
    let mut held = Vec::with_capacity(ancestors.len());
    for ancestor in ancestors {
        let directory = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(ancestor)
            .map_err(|_| ExportError::WriteFailed)?;
        let metadata = directory.metadata().map_err(|_| ExportError::WriteFailed)?;
        if !metadata.is_dir() || is_link(&metadata) {
            return Err(ExportError::InvalidDestination);
        }
        held.push(directory);
    }
    Ok(held)
}

#[cfg(windows)]
fn is_link(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(windows)]
fn existing_destination(metadata: &Metadata) -> ExportError {
    if metadata.is_file() && !is_link(metadata) {
        ExportError::DestinationExists
    } else {
        ExportError::InvalidDestination
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn ancestor_pins_exclude_writable_directory_handles_in_both_orders() {
        use std::os::windows::fs::OpenOptionsExt;
        let temp = tempfile::tempdir().expect("owned namespace");
        let directory = temp.path().join("empty ancestor");
        std::fs::create_dir(&directory).expect("ancestor");
        let destination = directory.join("history.txt");
        let writable = || {
            OpenOptions::new()
                .read(true)
                .write(true)
                .share_mode(0x7)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&directory)
        };
        let mutator = writable().expect("baseline writable directory handle");
        assert!(
            hold_plain_ancestors(&destination).is_err(),
            "an existing writable handle must prevent pinning"
        );
        drop(mutator);
        let _pins = hold_plain_ancestors(&destination).expect("plain ancestors pinned");
        assert_eq!(
            writable()
                .expect_err("pin excludes a new reparse mutator")
                .raw_os_error(),
            Some(32)
        );
        assert!(!destination.exists());
    }

    #[cfg(not(all(windows, feature = "folder_dialog")))]
    #[test]
    fn unavailable_picker_is_explicit_without_a_dialog() {
        assert_eq!(
            pick_export_destination(ExportFormat::Json),
            Err(ExportError::PickerUnsupported)
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn writing_is_explicitly_unsupported_off_windows() {
        assert_eq!(
            write_export(Path::new("/conversation.json"), b"{}", ExportFormat::Json),
            Err(ExportError::PickerUnsupported)
        );
    }

    #[cfg(windows)]
    #[test]
    fn new_unicode_exports_preserve_exact_bytes_and_accept_extension_case() {
        let temp = tempfile::tempdir().expect("fixture");
        for (name, format, bytes) in [
            (
                "conversation 中文.JSON",
                ExportFormat::Json,
                b"{\"text\":\"private\"}".as_slice(),
            ),
            (
                "conversation 日本語.TXT",
                ExportFormat::Text,
                "private body 中文\n".as_bytes(),
            ),
        ] {
            let path = temp.path().join(name);
            assert_eq!(write_export(&path, bytes, format), Ok(()));
            assert_eq!(std::fs::read(&path).expect("saved export"), bytes);
        }
    }

    #[cfg(windows)]
    #[test]
    fn existing_files_are_never_overwritten() {
        let temp = tempfile::tempdir().expect("fixture");
        let path = temp.path().join("conversation.json");
        let original = b"existing private content";
        std::fs::write(&path, original).expect("existing file");
        assert_eq!(
            write_export(&path, b"replacement", ExportFormat::Json),
            Err(ExportError::DestinationExists)
        );
        assert_eq!(std::fs::read(&path).expect("unchanged file"), original);
    }

    #[cfg(windows)]
    #[test]
    fn directory_wrong_extension_relative_and_dot_paths_are_refused() {
        let temp = tempfile::tempdir().expect("fixture");
        let directory = temp.path().join("directory.json");
        std::fs::create_dir(&directory).expect("directory");
        for path in [
            directory,
            temp.path().join("conversation.txt"),
            PathBuf::from("conversation.json"),
            temp.path().join("..").join("conversation.json"),
            temp.path().join(".").join("conversation.json"),
        ] {
            assert_eq!(
                write_export(&path, b"{}", ExportFormat::Json),
                Err(ExportError::InvalidDestination)
            );
        }
        assert!(!temp.path().join("conversation.txt").exists());
        assert!(!temp.path().join("conversation.json").exists());
    }

    #[cfg(windows)]
    #[test]
    fn oversized_export_is_refused_before_creating_a_file() {
        let temp = tempfile::tempdir().expect("fixture");
        let path = temp.path().join("conversation.json");
        let length = usize::try_from(MAX_EXPORT_BYTES + 1).expect("export limit fits platform");
        let bytes = vec![b'x'; length];
        assert_eq!(
            write_export(&path, &bytes, ExportFormat::Json),
            Err(ExportError::Limit)
        );
        assert!(!path.exists());
    }

    #[cfg(windows)]
    #[test]
    fn windows_reserved_ads_and_ambiguous_names_are_refused() {
        let temp = tempfile::tempdir().expect("fixture");
        for name in [
            "NUL.json",
            "con.any.json",
            "COM1.json",
            "LPT9.json",
            "COM¹.json",
            "LPT².json",
            "CONIN$.json",
            "CONOUT$.json",
            "conversation:stream.json",
            "conversation.json ",
            "conversation.json.",
            "folder.\\conversation.json",
            "folder \\conversation.json",
            "NUL\\conversation.json",
            "NUL .json",
            "bad|name.json",
        ] {
            assert_eq!(
                write_export(&temp.path().join(name), b"{}", ExportFormat::Json),
                Err(ExportError::InvalidDestination),
                "{name}"
            );
        }
        for path in [
            r"\\.\C:\conversation.json",
            r"\\?\C:\conversation.json",
            r"C:conversation.json",
            r"\conversation.json",
        ] {
            assert_eq!(
                write_export(Path::new(path), b"{}", ExportFormat::Json),
                Err(ExportError::InvalidDestination)
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn junction_ancestors_are_refused_without_writing_the_target() {
        use std::os::windows::process::CommandExt;
        let temp = tempfile::tempdir().expect("fixture");
        let target = temp.path().join("target");
        let link = temp.path().join("linked");
        std::fs::create_dir(&target).expect("target");
        let powershell = PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"))
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let status = std::process::Command::new(powershell)
            .args(["-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:VIBEMUX_TEST_LINK -Target $env:VIBEMUX_TEST_TARGET | Out-Null"])
            .env("VIBEMUX_TEST_LINK", &link).env("VIBEMUX_TEST_TARGET", &target)
            .creation_flags(0x0800_0000).status().expect("junction fixture");
        assert!(status.success());
        let result = write_export(&link.join("conversation.json"), b"{}", ExportFormat::Json);
        std::fs::remove_dir(&link).expect("remove only the owned junction");
        assert_eq!(result, Err(ExportError::InvalidDestination));
        assert!(!target.join("conversation.json").exists());
    }
}
