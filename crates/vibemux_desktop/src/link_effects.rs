//! Native desktop handoff and a text-only clipboard owned by the application.
use crate::command_error::CommandError;
use std::{path::Path, sync::Mutex};
use url::Url;

/// First-party native composition seam. Renderer input cannot select an adapter.
pub trait LinkEffects: Send + Sync {
    fn open_https(&self, url: &Url) -> Result<(), CommandError>;
    fn open_directory(&self, directory: &Path) -> Result<(), CommandError>;
    fn copy_text(&self, text: &str) -> Result<(), CommandError>;
}

#[derive(Default)]
pub struct SystemLinkEffects {
    // Keep Linux clipboard selection ownership for the application lifetime.
    // All operations are serialized; the clipboard is never read or restored.
    clipboard: Mutex<Option<arboard::Clipboard>>,
}

impl LinkEffects for SystemLinkEffects {
    fn open_https(&self, url: &Url) -> Result<(), CommandError> {
        open::that_detached(url.as_str()).map_err(|_| open_failed())
    }

    fn open_directory(&self, directory: &Path) -> Result<(), CommandError> {
        open::that_detached(directory).map_err(|_| open_failed())
    }

    fn copy_text(&self, text: &str) -> Result<(), CommandError> {
        let mut clipboard = self.clipboard.lock().map_err(|_| clipboard_failed())?;
        if clipboard.is_none() {
            *clipboard = Some(arboard::Clipboard::new().map_err(|_| clipboard_failed())?);
        }
        clipboard
            .as_mut()
            .ok_or_else(clipboard_failed)?
            .set_text(text.to_owned())
            .map_err(|_| clipboard_failed())
    }
}

fn open_failed() -> CommandError {
    CommandError::new(
        "link_open_unconfirmed",
        "The default application did not confirm the open request.",
    )
}

fn clipboard_failed() -> CommandError {
    CommandError::new(
        "link_clipboard_unconfirmed",
        "The clipboard operation could not be confirmed.",
    )
}
