//! Fixed empty terminal peer for the explicitly selected cleanup-review fixture.
use super::{FilePin, QaError, copy_image, create_file, verify_file};
use serde_json::json;
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};
use vibemuxd::{
    control::ControlClient, plugin_configuration::PluginStartup, plugin_registry::PluginState,
    process::DaemonPaths,
};

const IMAGE_NAME: &str = "vibemux_registry_mock.exe";
const MANIFEST_NAME: &str = "cleanup_terminal_manifest.toml";
const STARTUP_NAME: &str = "cleanup_terminal_startup.json";
const PLUGIN_ID: &str = "mock.harness";
const CLEANUP_METHOD: &str = "terminal:cleanup_inventory_v1";
const READY_TIMEOUT: Duration = Duration::from_secs(15);
const READY_INTERVAL: Duration = Duration::from_millis(50);
const MAX_IMAGE_BYTES: u64 = 256 * 1024 * 1024;
const IMAGE_BUFFER_BYTES: usize = 64 * 1024;
const MANIFEST: &[u8] = br#"schema_version = 1
plugin_id = "mock.harness"
plugin_version = "1.0.0"
kind = "terminal"
entry_point = ["vibemux_registry_mock", "terminal_cleanup_normal"]
capabilities = ["terminal:inventory_v1", "terminal:focus_v1", "terminal:cleanup_inventory_v1"]
requested_permissions = ["terminal:observe", "terminal:focus", "process:execute"]
supported_platforms = ["windows"]
protocol_major = 1
minimum_protocol_minor = 0
maximum_protocol_minor = 0
"#;

pub(super) struct CleanupFixture {
    startup: PathBuf,
    pins: Vec<FilePin>,
}

pub(super) fn prepare(vendor: &Path, chat_fixture: &Path) -> Result<(), QaError> {
    let source = chat_fixture.with_file_name(IMAGE_NAME);
    let _source_pin = FilePin::open(&source)?;
    copy_image(&source, &vendor.join(IMAGE_NAME))?;
    create_file(&vendor.join(MANIFEST_NAME), MANIFEST)?;
    create_file(&vendor.join(STARTUP_NAME), &startup_bytes(vendor)?)
}

impl CleanupFixture {
    pub(super) fn open(vendor: &Path, chat_fixture: &Path) -> Result<Self, QaError> {
        let source = chat_fixture.with_file_name(IMAGE_NAME);
        let image = vendor.join(IMAGE_NAME);
        let startup = vendor.join(STARTUP_NAME);
        let pins = [&source, &image, &vendor.join(MANIFEST_NAME), &startup]
            .iter()
            .map(|path| FilePin::open(path))
            .collect::<Result<Vec<_>, _>>()?;
        // Pins deny mutation while the complete source/copy relation is read.
        verify_image_copy(&source, &image)?;
        verify_file(&vendor.join(MANIFEST_NAME), MANIFEST)?;
        verify_file(&startup, &startup_bytes(vendor)?)?;
        let fixture = Self { startup, pins };
        fixture.verify()?;
        Ok(fixture)
    }

    pub(super) fn verify(&self) -> Result<(), QaError> {
        for pin in &self.pins {
            pin.verify()?;
        }
        Ok(())
    }

    pub(super) fn startup(&self) -> Result<PluginStartup, QaError> {
        self.verify()?;
        PluginStartup::from_path(&self.startup).map_err(|_| QaError::OwnershipChanged)
    }

    pub(super) async fn wait_ready(&self, paths: &DaemonPaths) -> Result<(), QaError> {
        let client = ControlClient::from_descriptor(paths.descriptor_path())
            .map_err(|_| QaError::RuntimeUnavailable)?;
        tokio::time::timeout(READY_TIMEOUT, async {
            loop {
                let statuses = client
                    .plugin_status()
                    .await
                    .map_err(|_| QaError::RuntimeUnavailable)?;
                match statuses.as_slice() {
                    [status]
                        if status.plugin_id == PLUGIN_ID && status.state == PluginState::Active =>
                    {
                        return Ok(());
                    }
                    [] => {}
                    [status]
                        if status.plugin_id == PLUGIN_ID
                            && status.state == PluginState::Starting => {}
                    _ => return Err(QaError::RuntimeUnavailable),
                }
                tokio::time::sleep(READY_INTERVAL).await;
            }
        })
        .await
        .map_err(|_| QaError::RuntimeUnavailable)?
    }
}

fn startup_bytes(vendor: &Path) -> Result<Vec<u8>, QaError> {
    serde_json::to_vec_pretty(&json!({
        "schema_version": 2, "max_plugins": 1, "plugins": [{
            "manifest_path": vendor.join(MANIFEST_NAME), "executable": vendor.join(IMAGE_NAME),
            "working_directory": vendor,
            "granted_capabilities": [CLEANUP_METHOD],
            "granted_permissions": ["terminal:observe", "process:execute"],
            "restart": {"max_restarts": 0, "initial_backoff_ms": 100, "max_backoff_ms": 100}
        }]
    }))
    .map_err(|_| QaError::Io)
}

fn verify_image_copy(source: &Path, image: &Path) -> Result<(), QaError> {
    let mut source = File::open(source).map_err(|_| QaError::FixtureUnavailable)?;
    let mut image = File::open(image).map_err(|_| QaError::FixtureUnavailable)?;
    let length = source
        .metadata()
        .map_err(|_| QaError::FixtureUnavailable)?
        .len();
    if length == 0
        || length > MAX_IMAGE_BYTES
        || image
            .metadata()
            .map_err(|_| QaError::FixtureUnavailable)?
            .len()
            != length
    {
        return Err(QaError::FixtureUnavailable);
    }
    let mut source_bytes = [0; IMAGE_BUFFER_BYTES];
    let mut image_bytes = [0; IMAGE_BUFFER_BYTES];
    let mut remaining = length;
    while remaining != 0 {
        let count = usize::try_from(remaining.min(IMAGE_BUFFER_BYTES as u64))
            .map_err(|_| QaError::FixtureUnavailable)?;
        source
            .read_exact(&mut source_bytes[..count])
            .map_err(|_| QaError::FixtureUnavailable)?;
        image
            .read_exact(&mut image_bytes[..count])
            .map_err(|_| QaError::FixtureUnavailable)?;
        if source_bytes[..count] != image_bytes[..count] {
            return Err(QaError::FixtureUnavailable);
        }
        remaining -= count as u64;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_relation_rejects_same_length_changes_and_empty_copies() {
        let temp = tempfile::tempdir().expect("fixture");
        let source = temp.path().join("source.exe");
        let image = temp.path().join("copy.exe");
        std::fs::write(&source, b"fixed image").expect("source");
        std::fs::write(&image, b"fixed image").expect("copy");
        verify_image_copy(&source, &image).expect("equal bytes");
        std::fs::write(&image, b"other image").expect("same-length mutation");
        assert_eq!(
            verify_image_copy(&source, &image),
            Err(QaError::FixtureUnavailable)
        );
        std::fs::write(&source, b"").expect("empty source");
        std::fs::write(&image, b"").expect("empty copy");
        assert_eq!(
            verify_image_copy(&source, &image),
            Err(QaError::FixtureUnavailable)
        );
    }
}
