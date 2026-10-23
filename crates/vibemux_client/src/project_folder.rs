#![forbid(unsafe_code)]
//! Project folder selection for the GUI: parsing a typed or pasted path,
//! checking that it names an existing directory, display names, and the
//! native folder picker on Windows.

use std::path::{Path, PathBuf};

/// Why a folder cannot be opened.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectFolderError {
    Empty,
    NotAbsolute,
    NotADirectory,
}

impl ProjectFolderError {
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::Empty => "Enter a folder path.",
            Self::NotAbsolute => "Enter a full path, such as C:\\Projects\\app.",
            Self::NotADirectory => "That folder does not exist.",
        }
    }
}

/// Parses a typed or pasted folder path. Surrounding whitespace and one
/// pair of surrounding double quotes (as Explorer's "Copy as path" adds)
/// are removed. The path must be absolute; whether it exists is checked by
/// [`existing_folder`].
pub fn parse_folder_input(input: &str) -> Result<PathBuf, ProjectFolderError> {
    let trimmed = input.trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .unwrap_or(trimmed)
        .trim();
    if unquoted.is_empty() {
        return Err(ProjectFolderError::Empty);
    }
    let path = PathBuf::from(unquoted);
    if !path.is_absolute() {
        return Err(ProjectFolderError::NotAbsolute);
    }
    Ok(path)
}

/// `path` when it names an existing directory. The path is kept as given,
/// not canonicalized, so it matches what the CLI receives for the same
/// folder.
pub fn existing_folder(path: &Path) -> Result<PathBuf, ProjectFolderError> {
    if path.is_dir() {
        Ok(path.to_path_buf())
    } else {
        Err(ProjectFolderError::NotADirectory)
    }
}

/// The folder's own name for lists and headings, or the whole path for a
/// drive root.
#[must_use]
pub fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.display().to_string())
}

/// The folder the GUI observes at start: the saved active project while it
/// still exists, otherwise `fallback` (the directory the GUI started in).
#[must_use]
pub fn startup_folder(active: Option<&Path>, fallback: Option<PathBuf>) -> Option<PathBuf> {
    active
        .filter(|folder| folder.is_dir())
        .map(Path::to_path_buf)
        .or(fallback)
}

/// Whether this platform offers a native folder picker.
#[must_use]
pub const fn native_picker_available() -> bool {
    cfg!(windows)
}

/// Shows the native folder picker, starting in `start` when given.
/// Blocks until the user picks a folder or cancels. Returns `None` on
/// cancel and on platforms without a native picker.
#[cfg(feature = "folder_dialog")]
#[must_use]
pub fn pick_folder_native(start: Option<&Path>) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let mut dialog = rfd::FileDialog::new().set_title("Open a project folder");
        if let Some(start) = start.filter(|folder| folder.is_dir()) {
            dialog = dialog.set_directory(start);
        }
        dialog.pick_folder()
    }
    #[cfg(not(windows))]
    {
        let _ = start;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_and_pasted_paths_are_trimmed_and_unquoted() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let text = directory.path().display().to_string();
        for input in [
            text.clone(),
            format!("  {text}  "),
            format!("\"{text}\""),
            format!(" \"{text}\" "),
        ] {
            assert_eq!(
                parse_folder_input(&input).as_deref(),
                Ok(directory.path()),
                "{input}"
            );
        }
    }

    #[test]
    fn empty_and_relative_paths_are_refused() {
        assert_eq!(parse_folder_input("   "), Err(ProjectFolderError::Empty));
        assert_eq!(parse_folder_input("\"\""), Err(ProjectFolderError::Empty));
        assert_eq!(
            parse_folder_input("relative/folder"),
            Err(ProjectFolderError::NotAbsolute)
        );
    }

    #[test]
    fn only_existing_directories_are_folders() {
        let directory = tempfile::tempdir().expect("temporary directory");
        assert_eq!(
            existing_folder(directory.path()).as_deref(),
            Ok(directory.path())
        );
        let file = directory.path().join("file.txt");
        std::fs::write(&file, "x").expect("file");
        assert_eq!(
            existing_folder(&file),
            Err(ProjectFolderError::NotADirectory)
        );
        let missing = directory.path().join("missing");
        assert_eq!(
            existing_folder(&missing),
            Err(ProjectFolderError::NotADirectory)
        );
    }

    #[test]
    fn the_startup_folder_prefers_an_existing_saved_project() {
        let saved = tempfile::tempdir().expect("saved folder");
        let fallback = PathBuf::from("fallback");
        assert_eq!(
            startup_folder(Some(saved.path()), Some(fallback.clone())).as_deref(),
            Some(saved.path())
        );
        let gone = saved.path().join("gone");
        assert_eq!(
            startup_folder(Some(&gone), Some(fallback.clone())),
            Some(fallback.clone())
        );
        assert_eq!(startup_folder(None, Some(fallback.clone())), Some(fallback));
        assert_eq!(startup_folder(None, None), None);
    }

    #[test]
    fn display_names_use_the_last_component() {
        let path = PathBuf::from("projects").join("vibemux");
        assert_eq!(display_name(&path), "vibemux");
        assert!(
            ProjectFolderError::NotADirectory
                .message()
                .contains("exist")
        );
    }
}
