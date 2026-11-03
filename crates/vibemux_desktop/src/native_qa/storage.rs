//! Fresh owned QA storage. Unknown roots and files are retained, never removed.
use super::{QA_SCHEMA_VERSION, QaError, QaScenario, SeedIds, SeedSummary};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};
use uuid::Uuid;

pub(super) const SETTINGS_FILE: &str = "desktop_settings.json";
const ROOT_FILE: &str = "native_qa_root.json";
pub(super) const PLAN_FILE: &str = "native_qa_plan.json";
pub(super) const RUN_FILE: &str = "native_qa_run.json";
pub(super) const STOP_FILE: &str = "native_qa_stop.json";
pub(super) const STOP_STAGE_PREFIX: &str = "native_qa_stop_stage_";
pub(super) const OUTCOME_FILE: &str = "native_qa_outcome.json";
const MAX_RECORD_BYTES: u64 = 32 * 1024;
const REPARSE_POINT: u32 = 0x400;
const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const BACKUP_SEMANTICS: u32 = 0x0200_0000;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RootMarker {
    schema_version: u32,
    root_id: Uuid,
    created_nanos: u128,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct QaPlan {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario: Option<QaScenario>,
    pub root_id: Uuid,
    pub ids: SeedIds,
    pub baseline: SeedSummary,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct QaRun {
    pub schema_version: u32,
    pub root_id: Uuid,
    pub run_id: Uuid,
    pub runner_pid: u32,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StopRequest {
    pub schema_version: u32,
    pub root_id: Uuid,
    pub run_id: Uuid,
}

pub(super) struct OwnedRoot {
    path: PathBuf,
    identity: Arc<same_file::Handle>,
    marker: RootMarker,
    _pins: Vec<File>,
    children: Vec<DirectoryPin>,
}
impl OwnedRoot {
    pub fn create(path: PathBuf) -> Result<Self, QaError> {
        validate_path(&path)?;
        let mut pins = hold_ancestors(&path)?;
        std::fs::create_dir(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                QaError::RootExists
            } else {
                QaError::Io
            }
        })?;
        pins.push(open_directory(&path)?);
        let created_nanos = created(&path)?;
        let marker = RootMarker {
            schema_version: QA_SCHEMA_VERSION,
            root_id: Uuid::new_v4(),
            created_nanos,
        };
        let mut root = Self {
            identity: Arc::new(
                same_file::Handle::from_path(&path).map_err(|_| QaError::InvalidRoot)?,
            ),
            path,
            marker,
            _pins: pins,
            children: Vec::new(),
        };
        root.write_new(ROOT_FILE, &root.marker)?;
        std::fs::create_dir(root.path.join("profile")).map_err(|_| QaError::Io)?;
        std::fs::create_dir(root.path.join("webview")).map_err(|_| QaError::Io)?;
        std::fs::create_dir(root.path.join("exports")).map_err(|_| QaError::Io)?;
        root.pin_children()?;
        Ok(root)
    }
    pub fn open(path: PathBuf) -> Result<Self, QaError> {
        validate_path(&path)?;
        let mut pins = hold_ancestors(&path)?;
        pins.push(open_directory(&path)?);
        let marker: RootMarker = read_json(&path.join(ROOT_FILE))?;
        if marker.schema_version != QA_SCHEMA_VERSION
            || marker.root_id.is_nil()
            || marker.created_nanos != created(&path)?
        {
            return Err(QaError::OwnershipChanged);
        }
        let mut root = Self {
            identity: Arc::new(
                same_file::Handle::from_path(&path).map_err(|_| QaError::InvalidRoot)?,
            ),
            path,
            marker,
            _pins: pins,
            children: Vec::new(),
        };
        root.pin_children()?;
        root.verify()?;
        Ok(root)
    }
    fn pin_children(&mut self) -> Result<(), QaError> {
        self.children = ["profile", "webview", "exports"]
            .into_iter()
            .map(|name| DirectoryPin::open(&self.path.join(name)))
            .collect::<Result<_, _>>()?;
        Ok(())
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn id(&self) -> Uuid {
        self.marker.root_id
    }
    pub fn settings_path(&self) -> PathBuf {
        self.path.join("profile").join(SETTINGS_FILE)
    }
    pub fn read_settings(&self) -> Result<crate::desktop_settings::DesktopSettings, QaError> {
        self.child_directory("profile")?;
        read_json(&self.settings_path())
    }
    pub fn webview_path(&self) -> PathBuf {
        self.path.join("webview")
    }
    pub fn verify(&self) -> Result<(), QaError> {
        let current =
            same_file::Handle::from_path(&self.path).map_err(|_| QaError::OwnershipChanged)?;
        if current != *self.identity
            || created(&self.path)? != self.marker.created_nanos
            || read_json::<RootMarker>(&self.path.join(ROOT_FILE))? != self.marker
        {
            return Err(QaError::OwnershipChanged);
        }
        for child in &self.children {
            child.verify()?;
        }
        Ok(())
    }
    pub fn child_directory(&self, name: &str) -> Result<PathBuf, QaError> {
        basename(name)?;
        self.verify()?;
        let path = self.path.join(name);
        let metadata = std::fs::symlink_metadata(&path).map_err(|_| QaError::InvalidRoot)?;
        if !metadata.is_dir() || is_link(&metadata) {
            return Err(QaError::InvalidRoot);
        }
        Ok(path)
    }
    pub fn read<T: DeserializeOwned>(&self, name: &str) -> Result<T, QaError> {
        basename(name)?;
        self.verify()?;
        read_json(&self.path.join(name))
    }
    pub fn optional<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>, QaError> {
        basename(name)?;
        self.verify()?;
        match std::fs::symlink_metadata(self.path.join(name)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(QaError::Io),
            Ok(_) => self.read(name).map(Some),
        }
    }
    /// Publishes a complete stop without replacing a destination. A returned
    /// basename is an owned recovery link retained when post-publication deletion
    /// was denied; the final stop is already valid and publication succeeded.
    pub fn publish_stop(&self, request: &StopRequest) -> Result<Option<String>, QaError> {
        use std::os::windows::fs::OpenOptionsExt;
        self.verify()?;
        let run: QaRun = self.read(RUN_FILE)?;
        if request.schema_version != QA_SCHEMA_VERSION
            || request.root_id != self.id()
            || request.run_id.is_nil()
            || run.schema_version != QA_SCHEMA_VERSION
            || run.root_id != request.root_id
            || run.run_id != request.run_id
            || run.runner_pid == 0
        {
            return Err(QaError::OwnershipChanged);
        }
        if let Some(previous) = self.optional::<StopRequest>(STOP_FILE)? {
            return if previous == *request {
                Ok(None)
            } else {
                Err(QaError::OwnershipChanged)
            };
        }
        let bytes = serde_json::to_vec_pretty(request).map_err(|_| QaError::Io)?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(QaError::Io);
        }
        let stage_name = format!(
            "{STOP_STAGE_PREFIX}{}_{}.json",
            request.run_id,
            Uuid::new_v4()
        );
        let stage_path = self.path.join(&stage_name);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(0)
            .custom_flags(OPEN_REPARSE_POINT)
            .open(&stage_path)
            .map_err(|_| QaError::Io)?;
        let written = file.write_all(&bytes).and_then(|_| file.sync_all());
        drop(file); // No exclusive writer exists when the final name appears.
        if written.is_err() {
            if self.verify().is_ok() {
                let _ = std::fs::remove_file(&stage_path);
            }
            return Err(QaError::Io);
        }
        self.publish_stop_stage(stage_name, request)
    }

    /// The creator passes only its fresh, fully written and closed owned stage.
    /// This boundary publishes once or validates a competing matching stop.
    pub(super) fn publish_stop_stage(
        &self,
        stage_name: String,
        request: &StopRequest,
    ) -> Result<Option<String>, QaError> {
        basename(&stage_name)?;
        self.verify()?;
        let run: QaRun = self.read(RUN_FILE)?;
        if !stage_name.starts_with(STOP_STAGE_PREFIX)
            || request.schema_version != QA_SCHEMA_VERSION
            || request.root_id != self.id()
            || request.run_id.is_nil()
            || run.schema_version != QA_SCHEMA_VERSION
            || run.root_id != request.root_id
            || run.run_id != request.run_id
            || run.runner_pid == 0
        {
            return Err(QaError::OwnershipChanged);
        }
        let stage_path = self.path.join(&stage_name);
        if read_json::<StopRequest>(&stage_path)? != *request {
            return Err(QaError::OwnershipChanged);
        }
        // Both paths are in the same pinned local root. Hard-link creation is
        // atomic and fails if the destination already exists.
        let published = match std::fs::hard_link(&stage_path, self.path.join(STOP_FILE)) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if self.read::<StopRequest>(STOP_FILE)? == *request {
                    Ok(false)
                } else {
                    Err(QaError::OwnershipChanged)
                }
            }
            Err(_) => Err(QaError::Io),
        };
        let linked_final = match published {
            Ok(linked_final) => linked_final,
            Err(error) => {
                if self.verify().is_ok() {
                    let _ = std::fs::remove_file(&stage_path);
                }
                return Err(error);
            }
        };
        self.verify()?;
        if self.read::<StopRequest>(STOP_FILE)? != *request {
            return Err(QaError::OwnershipChanged);
        }
        self.cleanup_stop_stage(stage_name, request, linked_final)
    }

    fn cleanup_stop_stage(
        &self,
        stage_name: String,
        request: &StopRequest,
        linked_final: bool,
    ) -> Result<Option<String>, QaError> {
        basename(&stage_name)?;
        self.verify()?;
        if !stage_name.starts_with(STOP_STAGE_PREFIX)
            || request.schema_version != QA_SCHEMA_VERSION
            || request.root_id != self.id()
            || request.run_id.is_nil()
        {
            return Err(QaError::OwnershipChanged);
        }
        let stage_path = self.path.join(&stage_name);
        let metadata = match std::fs::symlink_metadata(&stage_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(QaError::OwnershipChanged),
        };
        if !metadata.is_file() || is_link(&metadata) {
            return Err(QaError::OwnershipChanged);
        }
        let final_path = self.path.join(STOP_FILE);
        // Only a newly linked winner must share the final inode. A matching
        // loser still owns its separate stage, validated by root and nonce.
        let same_file = !linked_final || {
            let stage =
                same_file::Handle::from_path(&stage_path).map_err(|_| QaError::OwnershipChanged)?;
            let published =
                same_file::Handle::from_path(&final_path).map_err(|_| QaError::OwnershipChanged)?;
            stage == published
        };
        if !same_file
            || read_json::<StopRequest>(&stage_path)? != *request
            || read_json::<StopRequest>(&final_path)? != *request
        {
            return Err(QaError::OwnershipChanged);
        }
        // A stage reader can deny DELETE sharing on this link. Retain
        // this known owned stage explicitly instead of reporting a failed stop.
        Ok(std::fs::remove_file(&stage_path).err().map(|_| stage_name))
    }

    pub fn write_new(&self, name: &str, value: &impl Serialize) -> Result<(), QaError> {
        use std::os::windows::fs::OpenOptionsExt;
        basename(name)?;
        // Initial root marker creation precedes verify; every other write is bound.
        if name != ROOT_FILE {
            self.verify()?;
        }
        let bytes = serde_json::to_vec_pretty(value).map_err(|_| QaError::Io)?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(QaError::Io);
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(0)
            .custom_flags(OPEN_REPARSE_POINT)
            .open(self.path.join(name))
            .map_err(|_| QaError::Io)?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| QaError::Io)
    }
}
// Mutable directory consumers use ordinary Windows paths. These handles exclude
// replacement while allowing normal file creation/atomic saves. External in-place
// namespace mutation is unsupported; identity/reparse checks detect observed changes.
pub(super) struct DirectoryPin {
    path: PathBuf,
    identity: same_file::Handle,
}
impl DirectoryPin {
    pub fn open(path: &Path) -> Result<Self, QaError> {
        use std::os::windows::fs::OpenOptionsExt;
        let file = OpenOptions::new()
            .read(true)
            .share_mode(0x3)
            .custom_flags(BACKUP_SEMANTICS | OPEN_REPARSE_POINT)
            .open(path)
            .map_err(|_| QaError::OwnershipChanged)?;
        let metadata = file.metadata().map_err(|_| QaError::OwnershipChanged)?;
        if !metadata.is_dir() || is_link(&metadata) {
            return Err(QaError::OwnershipChanged);
        }
        let pin = Self {
            path: path.to_owned(),
            identity: same_file::Handle::from_file(file).map_err(|_| QaError::OwnershipChanged)?,
        };
        pin.verify()?;
        Ok(pin)
    }
    pub fn verify(&self) -> Result<(), QaError> {
        let metadata =
            std::fs::symlink_metadata(&self.path).map_err(|_| QaError::OwnershipChanged)?;
        let current =
            same_file::Handle::from_path(&self.path).map_err(|_| QaError::OwnershipChanged)?;
        if !metadata.is_dir() || is_link(&metadata) || current != self.identity {
            return Err(QaError::OwnershipChanged);
        }
        Ok(())
    }
}
// Immutable fixture inputs cannot be written, deleted or replaced while consumed.
pub(super) struct FilePin {
    path: PathBuf,
    identity: same_file::Handle,
    length: u64,
}
impl FilePin {
    pub fn open(path: &Path) -> Result<Self, QaError> {
        use std::os::windows::fs::OpenOptionsExt;
        let file = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .custom_flags(OPEN_REPARSE_POINT)
            .open(path)
            .map_err(|_| QaError::OwnershipChanged)?;
        let metadata = file.metadata().map_err(|_| QaError::OwnershipChanged)?;
        if !metadata.is_file() || is_link(&metadata) {
            return Err(QaError::OwnershipChanged);
        }
        let pin = Self {
            path: path.to_owned(),
            length: metadata.len(),
            identity: same_file::Handle::from_file(file).map_err(|_| QaError::OwnershipChanged)?,
        };
        pin.verify()?;
        Ok(pin)
    }
    pub fn verify(&self) -> Result<(), QaError> {
        let metadata =
            std::fs::symlink_metadata(&self.path).map_err(|_| QaError::OwnershipChanged)?;
        let current =
            same_file::Handle::from_path(&self.path).map_err(|_| QaError::OwnershipChanged)?;
        if !metadata.is_file()
            || is_link(&metadata)
            || metadata.len() != self.length
            || current != self.identity
        {
            return Err(QaError::OwnershipChanged);
        }
        Ok(())
    }
}
fn basename(name: &str) -> Result<(), QaError> {
    if Path::new(name).components().count() != 1
        || !matches!(
            Path::new(name).components().next(),
            Some(Component::Normal(_))
        )
    {
        Err(QaError::InvalidRoot)
    } else {
        Ok(())
    }
}
fn validate_path(path: &Path) -> Result<(), QaError> {
    use std::path::Prefix;
    if !path.is_absolute()
        || path.file_name().is_none()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        || !matches!(path.components().next(),Some(Component::Prefix(p))if matches!(p.kind(),Prefix::Disk(_)))
    {
        return Err(QaError::InvalidRoot);
    }
    Ok(())
}
fn created(path: &Path) -> Result<u128, QaError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| QaError::InvalidRoot)?;
    if !metadata.is_dir() || is_link(&metadata) {
        return Err(QaError::InvalidRoot);
    }
    metadata
        .created()
        .map_err(|_| QaError::InvalidRoot)?
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .map_err(|_| QaError::InvalidRoot)
}
fn is_link(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_type().is_symlink() || metadata.file_attributes() & REPARSE_POINT != 0
}
fn open_directory(path: &Path) -> Result<File, QaError> {
    use std::os::windows::fs::OpenOptionsExt;
    let file = OpenOptions::new()
        .read(true)
        // Directory writes include creating a hard link. Continue denying
        // DELETE sharing so the pinned namespace cannot be renamed or removed.
        .share_mode(0x3)
        .custom_flags(BACKUP_SEMANTICS | OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|_| QaError::InvalidRoot)?;
    let metadata = file.metadata().map_err(|_| QaError::InvalidRoot)?;
    if !metadata.is_dir() || is_link(&metadata) {
        return Err(QaError::InvalidRoot);
    }
    Ok(file)
}
fn hold_ancestors(path: &Path) -> Result<Vec<File>, QaError> {
    let mut ancestors = path
        .parent()
        .ok_or(QaError::InvalidRoot)?
        .ancestors()
        .collect::<Vec<_>>();
    ancestors.reverse();
    ancestors.into_iter().map(open_directory).collect()
}
fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, QaError> {
    use std::os::windows::fs::OpenOptionsExt;
    let file = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .custom_flags(OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|_| QaError::InvalidRoot)?;
    let metadata = file.metadata().map_err(|_| QaError::InvalidRoot)?;
    if !metadata.is_file() || is_link(&metadata) || metadata.len() > MAX_RECORD_BYTES {
        return Err(QaError::InvalidRoot);
    }
    let mut bytes = Vec::new();
    file.take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| QaError::InvalidRoot)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(QaError::InvalidRoot);
    }
    serde_json::from_slice(&bytes).map_err(|_| QaError::InvalidRoot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_settings_support_the_production_atomic_replace_sequence() {
        let temp = tempfile::tempdir().expect("fixture");
        let root = OwnedRoot::create(temp.path().join("settings root")).expect("root");
        let settings = root.settings_path();
        let temporary = settings.with_extension("json.tmp");
        let directory = std::fs::create_dir_all(root.path());
        let written = std::fs::write(&temporary, b"{}");
        let renamed = std::fs::rename(&temporary, &settings);
        assert!(directory.is_ok() && written.is_ok() && renamed.is_ok());
        let mut value = crate::desktop_settings::DesktopSettings::default();
        crate::desktop_settings::save_settings_at(&settings, &value).expect("production save");
        value.appearance = crate::desktop_settings::Appearance::Dark;
        crate::desktop_settings::save_settings_at(&settings, &value)
            .expect("production atomic replace");
        assert_eq!(root.read_settings().expect("owned settings"), value);
    }

    #[test]
    fn child_pins_retain_identity_and_preserve_atomic_settings() {
        use std::os::windows::fs::OpenOptionsExt;
        let temp = tempfile::tempdir().expect("fixture");
        let root = OwnedRoot::create(temp.path().join("child pins")).expect("root");
        let profile = root.child_directory("profile").expect("profile");
        let pin = OpenOptions::new()
            .read(true)
            .share_mode(0x3)
            .custom_flags(BACKUP_SEMANTICS | OPEN_REPARSE_POINT)
            .open(&profile)
            .expect("compatible pin");
        let deletion = OpenOptions::new()
            .access_mode(0x10000)
            .share_mode(0x7)
            .custom_flags(BACKUP_SEMANTICS | OPEN_REPARSE_POINT)
            .open(&profile);
        assert!(deletion.is_err(), "held child excludes replacement");
        let mut value = crate::desktop_settings::DesktopSettings::default();
        crate::desktop_settings::save_settings_at(&root.settings_path(), &value)
            .expect("create settings");
        value.appearance = crate::desktop_settings::Appearance::Dark;
        crate::desktop_settings::save_settings_at(&root.settings_path(), &value)
            .expect("replace settings");
        assert_eq!(root.read_settings().expect("read"), value);
        drop(deletion);
        drop(pin);
    }
    #[test]
    fn immutable_fixture_pins_allow_reads_but_refuse_writes_and_replacement() {
        let temp = tempfile::tempdir().expect("fixture");
        let path = temp.path().join("fixture_config.json");
        std::fs::write(&path, b"fixed configuration").expect("config");
        let pin = FilePin::open(&path).expect("pin");
        assert_eq!(
            std::fs::read(&path).expect("consumer read"),
            b"fixed configuration"
        );
        assert_eq!(
            std::fs::write(&path, b"changed")
                .expect_err("immutable input")
                .raw_os_error(),
            Some(32)
        );
        assert_eq!(
            std::fs::rename(&path, temp.path().join("replacement.json"))
                .expect_err("retained path")
                .raw_os_error(),
            Some(32)
        );
        pin.verify().expect("identity retained");
        drop(pin);
        std::fs::write(&path, b"changed").expect("pin released after consumer");
    }
    #[test]
    fn fresh_unicode_root_is_bound_existing_roots_and_traversal_are_refused() {
        let temp = tempfile::tempdir().expect("fixture");
        let path = temp.path().join("DEMO QA 空间");
        let root = OwnedRoot::create(path.clone()).expect("fresh");
        let id = root.id();
        assert!(matches!(
            OwnedRoot::create(path.clone()),
            Err(QaError::RootExists)
        ));
        assert!(root.write_new("../outside.json", &true).is_err());
        root.verify().expect("bound");
        drop(root);
        let opened = OwnedRoot::open(path).expect("reopen");
        assert_eq!(opened.id(), id);
    }
    #[test]
    fn replaced_or_corrupt_marker_is_not_adopted() {
        let temp = tempfile::tempdir().expect("fixture");
        let path = temp.path().join("root");
        let root = OwnedRoot::create(path.clone()).expect("fresh");
        drop(root);
        std::fs::write(path.join(ROOT_FILE), b"{}").expect("owned corruption");
        assert!(OwnedRoot::open(path).is_err());
    }
    #[test]
    fn owned_stage_cleanup_retains_a_link_while_a_stage_consumer_denies_delete_sharing() {
        let temp = tempfile::tempdir().expect("fixture");
        let root = OwnedRoot::create(temp.path().join("blocked stage cleanup")).expect("root");
        let run = QaRun {
            schema_version: QA_SCHEMA_VERSION,
            root_id: root.id(),
            run_id: Uuid::new_v4(),
            runner_pid: std::process::id(),
        };
        root.write_new(RUN_FILE, &run).expect("run");
        let request = StopRequest {
            schema_version: QA_SCHEMA_VERSION,
            root_id: root.id(),
            run_id: run.run_id,
        };
        let stage_name = format!("{STOP_STAGE_PREFIX}{}_{}.json", run.run_id, Uuid::new_v4());
        root.write_new(&stage_name, &request)
            .expect("owned complete stage");
        let stage = root.path().join(&stage_name);
        std::fs::hard_link(&stage, root.path().join(STOP_FILE)).expect("atomic publication");
        let consumer = FilePin::open(&stage).expect("stage consumer denies delete sharing");
        assert_eq!(
            root.cleanup_stop_stage(stage_name.clone(), &request, true)
                .expect("publication stays valid"),
            Some(stage_name.clone())
        );
        assert!(stage.is_file());
        assert_eq!(
            root.read::<StopRequest>(STOP_FILE)
                .expect("valid final stop"),
            request
        );
        assert_eq!(
            root.publish_stop(&request)
                .expect("matching idempotent stop"),
            None
        );
        drop(consumer);
        assert_eq!(
            root.cleanup_stop_stage(stage_name, &request, true)
                .expect("cleanup after consumer closes"),
            None
        );
        assert!(!stage.exists());
        assert_eq!(
            root.read::<StopRequest>(STOP_FILE)
                .expect("final link remains"),
            request
        );
    }

    #[test]
    fn directory_pins_allow_link_publication_without_allowing_namespace_rename() {
        let temp = tempfile::tempdir().expect("owned fixture");
        let path = temp.path().join("pinned");
        let moved = temp.path().join("moved");
        let root = OwnedRoot::create(path.clone()).expect("root");
        let source = root.path().join("closed_source.json");
        let published = root.path().join("published.json");
        std::fs::write(&source, b"fixed synthetic record").expect("closed source");
        std::fs::hard_link(&source, &published).expect("publication while pinned");
        assert_eq!(
            std::fs::read(&published).expect("record"),
            b"fixed synthetic record"
        );
        assert_eq!(
            std::fs::rename(&path, &moved)
                .expect_err("namespace remains pinned")
                .raw_os_error(),
            Some(32)
        );
        root.verify().expect("identity preserved");
        drop(root);
        std::fs::rename(&path, &moved).expect("rename after releasing only owned pins");
    }
}
