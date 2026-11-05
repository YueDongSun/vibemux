//! OS effects are recorded in memory; tests never open apps or touch a clipboard.
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};
use url::Url;
use vibemux_desktop::{command_error::CommandError, link_effects::LinkEffects};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LinkEffect {
    Https(String),
    Directory(PathBuf),
    Copy(String),
}

#[derive(Default)]
pub struct RecordedLinkEffects {
    pub events: Mutex<Vec<LinkEffect>>,
}

impl LinkEffects for RecordedLinkEffects {
    fn open_https(&self, url: &Url) -> Result<(), CommandError> {
        self.events
            .lock()
            .expect("effect recorder")
            .push(LinkEffect::Https(url.as_str().to_owned()));
        Ok(())
    }
    fn open_directory(&self, path: &Path) -> Result<(), CommandError> {
        self.events
            .lock()
            .expect("effect recorder")
            .push(LinkEffect::Directory(path.to_owned()));
        Ok(())
    }
    fn copy_text(&self, text: &str) -> Result<(), CommandError> {
        self.events
            .lock()
            .expect("effect recorder")
            .push(LinkEffect::Copy(text.to_owned()));
        Ok(())
    }
}
