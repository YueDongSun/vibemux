//! The verified control-client cache shared by the GUI's background reader
//! and the console's daemon feed. It never starts a daemon or recovers one.
use vibemuxd::{
    control::{ControlClient, ControlGeneration},
    process::DaemonPaths,
};

/// Connection state carried between polls so a healthy daemon is verified once
/// instead of once per tick.
#[derive(Default)]
pub(crate) struct DaemonSession {
    client: Option<ControlClient>,
    generation: Option<ControlGeneration>,
}

impl DaemonSession {
    pub(crate) fn invalidate(&mut self) {
        self.client = None;
        self.generation = None;
    }

    /// Return a usable client plus whether the runtime generation changed.
    ///
    /// The runtime ACL walk is the expensive part of connecting on Windows,
    /// so it runs once per generation: the descriptor is re-read every time
    /// (cheap, and the only way to notice a daemon that stopped), but the
    /// verification is redone only when the published descriptor is not the
    /// one already verified. Any call failure should `invalidate` the cache,
    /// so a dropped connection forces a fresh verification before it is
    /// trusted again.
    pub(crate) fn connect(&mut self, paths: &DaemonPaths) -> Result<(ControlClient, bool), String> {
        let published = read_published(paths)?;
        let generation = published.generation();
        if self.client.is_some() && self.generation.as_ref() == Some(&generation) {
            return Ok((published, false));
        }
        // Fail closed: nothing is used before the runtime behind it is verified.
        verify_runtime(paths)?;
        let changed = self
            .generation
            .as_ref()
            .is_some_and(|previous| *previous != generation);
        self.client = Some(published.clone());
        self.generation = Some(generation);
        Ok((published, changed))
    }
}

/// Read the published descriptor and resolve a client from it, without
/// re-verifying the runtime ACLs.
fn read_published(paths: &DaemonPaths) -> Result<ControlClient, String> {
    if !paths.descriptor_path().exists() {
        return Err("daemon_not_running".into());
    }
    ControlClient::from_descriptor(paths.descriptor_path())
        .map_err(|error| error.code().to_string())
}

/// Verify that the runtime this descriptor belongs to is still restricted.
fn verify_runtime(paths: &DaemonPaths) -> Result<(), String> {
    paths
        .validate_runtime_dir()
        .map_err(|error| error.code().to_string())?;
    #[cfg(windows)]
    paths
        .verify_control_security()
        .map_err(|error| error.code().to_string())?;
    Ok(())
}
