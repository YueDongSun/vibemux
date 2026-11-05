//! Retained identities for the fixed native bundle; no project exclusion policy.
use crate::harness_dispatch::native_process::LaunchPlan;
use std::{
    fs::{File, OpenOptions},
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf, Prefix},
};
use vibemux_harness::dispatch::launch_spec::LaunchSpec;
use vibemux_platform::{WindowsFileIdentity, query_windows_file_identity};

const SHARE_READ: u32 = 1;
const OPEN_REPARSE: u32 = 0x0020_0000;
const BACKUP_SEMANTICS: u32 = 0x0200_0000;
const REPARSE_ATTRIBUTE: u32 = 0x400;
const MAX_PINS: usize = 128;
const MAX_PATH_BYTES: usize = 4096;
const UNAVAILABLE: &str = "local_probe_artifact_unavailable";

struct Pin {
    path: PathBuf,
    file: File,
    identity: WindowsFileIdentity,
    length: Option<u64>,
}
impl Pin {
    fn acquire(path: &Path, directory: bool) -> Result<Self, &'static str> {
        validate_path(path)?;
        let file = open(path, directory)?;
        let metadata = file.metadata().map_err(|_| UNAVAILABLE)?;
        let identity = query_windows_file_identity(&file).map_err(|_| UNAVAILABLE)?;
        Ok(Self {
            path: path.to_owned(),
            file,
            identity,
            length: (!directory).then_some(metadata.len()),
        })
    }
    fn verify(&self) -> Result<(), &'static str> {
        let current = open(&self.path, self.length.is_none())?;
        if query_windows_file_identity(&self.file).map_err(|_| UNAVAILABLE)? != self.identity
            || query_windows_file_identity(&current).map_err(|_| UNAVAILABLE)? != self.identity
            || self.length.is_some_and(|length| {
                current
                    .metadata()
                    .map(|metadata| metadata.len() != length)
                    .unwrap_or(true)
            })
        {
            return Err(UNAVAILABLE);
        }
        Ok(())
    }
}

pub(super) struct Bundle {
    directory: PathBuf,
    probe: PathBuf,
    trampoline: PathBuf,
    pins: Vec<Pin>,
}
impl Bundle {
    pub(super) fn acquire() -> Result<Self, &'static str> {
        let image = std::env::current_exe().map_err(|_| UNAVAILABLE)?;
        Self::at(image.parent().ok_or(UNAVAILABLE)?)
    }
    fn at(directory: &Path) -> Result<Self, &'static str> {
        validate_path(directory)?;
        let mut ancestry = directory
            .ancestors()
            .map(Path::to_path_buf)
            .collect::<Vec<_>>();
        if ancestry.len() + 2 > MAX_PINS {
            return Err(UNAVAILABLE);
        }
        ancestry.reverse();
        let mut pins = ancestry
            .iter()
            .map(|path| Pin::acquire(path, true))
            .collect::<Result<Vec<_>, _>>()?;
        let directory = std::fs::canonicalize(directory).map_err(|_| UNAVAILABLE)?;
        validate_path(&directory)?;
        if Pin::acquire(&directory, true)?.identity != pins.last().ok_or(UNAVAILABLE)?.identity {
            return Err(UNAVAILABLE);
        }
        let probe = directory.join("vibemux_probe.exe");
        let trampoline = directory.join("vibemux_launch_trampoline.exe");
        pins.push(Pin::acquire(&probe, false)?);
        pins.push(Pin::acquire(&trampoline, false)?);
        let bundle = Self {
            directory,
            probe,
            trampoline,
            pins,
        };
        bundle.verify()?;
        Ok(bundle)
    }
    pub(super) fn verify(&self) -> Result<(), &'static str> {
        for pin in &self.pins {
            pin.verify()?;
        }
        Ok(())
    }
    pub(super) fn plan(&self) -> LaunchPlan {
        LaunchPlan {
            trampoline: self.trampoline.clone(),
            executable: self.probe.clone(),
            working_directory: self.directory.clone(),
            spec: LaunchSpec {
                executable: self.probe.clone(),
                arguments: Vec::new(),
                environment_names: Vec::new(),
            },
        }
    }
}

fn validate_path(path: &Path) -> Result<(), &'static str> {
    let text = path.to_str().ok_or(UNAVAILABLE)?;
    if !path.is_absolute()
        || text.len() > MAX_PATH_BYTES
        || text.chars().any(char::is_control)
        || !matches!(path.components().next(), Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
        || path.components().any(|component| match component {
            Component::ParentDir | Component::CurDir => true,
            Component::Normal(value) => value.to_string_lossy().contains(':'),
            _ => false,
        })
    {
        return Err(UNAVAILABLE);
    }
    Ok(())
}
fn open(path: &Path, directory: bool) -> Result<File, &'static str> {
    let file = OpenOptions::new()
        .read(true)
        .share_mode(SHARE_READ)
        .custom_flags(OPEN_REPARSE | if directory { BACKUP_SEMANTICS } else { 0 })
        .open(path)
        .map_err(|_| UNAVAILABLE)?;
    let metadata = file.metadata().map_err(|_| UNAVAILABLE)?;
    if metadata.file_attributes() & REPARSE_ATTRIBUTE != 0
        || (directory && !metadata.is_dir())
        || (!directory && (!metadata.is_file() || metadata.len() == 0))
    {
        return Err(UNAVAILABLE);
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let parent = tempfile::tempdir().expect("owned parent");
        let directory = parent.path().join("native bundle Ω");
        std::fs::create_dir(&directory).expect("bundle");
        for name in ["vibemux_probe.exe", "vibemux_launch_trampoline.exe"] {
            std::fs::write(directory.join(name), b"fixed synthetic image bytes").expect("image");
        }
        (parent, directory)
    }

    #[test]
    fn retained_bundle_denies_image_write_and_namespace_replacement() {
        let (parent, directory) = fixture();
        let bundle = Bundle::at(&directory).expect("immutable bundle");
        assert!(
            bundle.plan().spec.arguments.is_empty(),
            "no cache flags or operator argv"
        );
        assert_eq!(
            bundle.plan().working_directory,
            std::fs::canonicalize(&directory).expect("canonical bundle")
        );
        let image = directory.join("vibemux_probe.exe");
        assert!(OpenOptions::new().write(true).open(&image).is_err());
        let moved = parent.path().join("moved bundle");
        assert!(std::fs::rename(&directory, &moved).is_err());
        bundle.verify().expect("identities retained");
        drop(bundle);
        std::fs::rename(&directory, &moved).expect("release owned pins");
        std::fs::rename(&moved, &directory).expect("restore fixture for teardown");
    }

    #[test]
    fn an_existing_image_writer_blocks_admission_before_spawn() {
        let (_parent, directory) = fixture();
        let image = directory.join("vibemux_probe.exe");
        let writer = OpenOptions::new()
            .write(true)
            .share_mode(7)
            .open(&image)
            .expect("existing writable handle");
        assert!(Bundle::at(&directory).is_err());
        drop(writer);
        Bundle::at(&directory).expect("writer retired");
    }

    #[test]
    fn incomplete_bundle_and_unsupported_paths_refuse_without_creating_files() {
        let root = tempfile::tempdir().expect("owned root");
        assert!(Bundle::at(root.path()).is_err());
        assert_eq!(
            std::fs::read_dir(root.path()).expect("inventory").count(),
            0
        );
        for path in [
            "relative",
            "C:drive_relative",
            r"\\server\share\bundle",
            "C:/bundle:stream",
            "C:/bundle/../other",
        ] {
            assert!(validate_path(Path::new(path)).is_err(), "reject {path}");
        }
    }
}
