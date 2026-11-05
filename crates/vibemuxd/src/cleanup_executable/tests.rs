use super::*;

use std::{
    fs,
    os::windows::{fs::symlink_dir, process::CommandExt},
    process::Command,
};

const EXECUTABLE_BYTES: &[u8] = b"owned executable fixture";
const SENTINEL_BYTES: &[u8] = b"main project sentinel";

struct Fixture {
    _directory: tempfile::TempDir,
    bin: PathBuf,
    executable: PathBuf,
    project: PathBuf,
    sentinel: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("owned temporary directory");
        let bin = directory.path().join("tools/bin");
        let project = directory.path().join("project");
        fs::create_dir_all(&bin).expect("fixture binary directory");
        fs::create_dir(&project).expect("fixture project");
        let executable = bin.join("CleanupTool.EXE");
        fs::write(&executable, EXECUTABLE_BYTES).expect("fixture executable");
        let sentinel = project.join("sentinel.txt");
        fs::write(&sentinel, SENTINEL_BYTES).expect("project sentinel");
        Self {
            _directory: directory,
            bin,
            executable,
            project,
            sentinel,
        }
    }

    fn assert_intact(&self) {
        assert_eq!(
            fs::read(&self.executable).expect("unchanged executable"),
            EXECUTABLE_BYTES
        );
        assert_eq!(
            fs::read(&self.sentinel).expect("unchanged main project"),
            SENTINEL_BYTES
        );
    }
}

#[test]
fn stable_pin_accepts_case_and_disk_prefix_aliases_by_full_object_identity() {
    let fixture = Fixture::new();
    let pin = CleanupExecutablePin::acquire(&fixture.executable, &fixture.project).expect("pin");
    pin.verify().expect("stable pin");
    let canonical = fs::canonicalize(&fixture.executable).expect("canonical executable");
    assert_eq!(pin.canonical_path(), canonical);
    let platform =
        query_windows_file_identity(&File::open(&fixture.executable).expect("read handle"))
            .expect("platform identity");
    assert_eq!(
        pin.identity(),
        CleanupObjectIdentity::WindowsFileId128V1 {
            volume_serial_number: platform.volume_serial_number,
            file_id: platform.file_id,
        }
    );
    let case_alias = PathBuf::from(fixture.executable.to_string_lossy().to_ascii_uppercase());
    let case_project = PathBuf::from(fixture.project.to_string_lossy().to_ascii_uppercase());
    let alias = CleanupExecutablePin::acquire(&case_alias, &case_project).expect("case alias pin");
    assert_eq!(alias.identity(), pin.identity());
    alias.verify().expect("case alias stable");
    let verbatim =
        CleanupExecutablePin::acquire(&canonical, &fixture.project).expect("verbatim disk pin");
    assert_eq!(verbatim.identity(), pin.identity());
    fixture.assert_intact();
}

#[test]
fn retained_leaf_denies_write_and_rename_until_the_pin_is_dropped() {
    let fixture = Fixture::new();
    let pin = CleanupExecutablePin::acquire(&fixture.executable, &fixture.project).expect("pin");
    assert!(
        fs::write(&fixture.executable, b"replacement").is_err(),
        "leaf write must be refused"
    );
    let renamed = fixture.executable.with_file_name("renamed.exe");
    assert!(
        fs::rename(&fixture.executable, &renamed).is_err(),
        "leaf rename must be refused"
    );
    pin.verify().expect("failed mutations preserve pin");
    fixture.assert_intact();
    drop(pin);
    fs::rename(&fixture.executable, &renamed).expect("leaf rename after drop");
    fs::rename(&renamed, &fixture.executable).expect("restore owned fixture name");
    fs::write(&fixture.executable, EXECUTABLE_BYTES).expect("leaf write after drop");
    fixture.assert_intact();
}

#[test]
fn retained_ancestors_deny_writable_handles_until_the_pin_is_dropped() {
    let fixture = Fixture::new();
    let parent = fixture.bin.parent().expect("tools parent");
    let open_writable_directory = |path: &Path| {
        OpenOptions::new()
            .write(true)
            .share_mode(7)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
    };
    for directory in [&fixture.bin, parent, &fixture.project] {
        drop(open_writable_directory(directory).expect("owned directory is initially writable"));
    }
    let pin = CleanupExecutablePin::acquire(&fixture.executable, &fixture.project).expect("pin");
    for directory in [&fixture.bin, parent, &fixture.project] {
        assert!(
            open_writable_directory(directory).is_err(),
            "retained ancestry must deny write access"
        );
    }
    pin.verify().expect("failed writable opens preserve pin");
    fixture.assert_intact();
    drop(pin);
    for directory in [&fixture.bin, parent, &fixture.project] {
        drop(open_writable_directory(directory).expect("directory write access after drop"));
    }
    fixture.assert_intact();
}

#[test]
fn every_retained_ancestor_denies_rename_and_releases_after_drop() {
    let fixture = Fixture::new();
    let pin = CleanupExecutablePin::acquire(&fixture.executable, &fixture.project).expect("pin");
    let parent = fixture.bin.parent().expect("tools parent");
    for directory in [&fixture.bin, parent, &fixture.project] {
        let renamed = directory.with_extension("moved");
        assert!(
            fs::rename(directory, &renamed).is_err(),
            "retained directory rename must be refused"
        );
    }
    pin.verify().expect("ancestry remains stable");
    drop(pin);
    for directory in [&fixture.bin, parent, &fixture.project] {
        let renamed = directory.with_extension("moved");
        fs::rename(directory, &renamed).expect("directory rename after drop");
        fs::rename(&renamed, directory).expect("restore owned directory name");
    }
    fixture.assert_intact();
}

fn create_directory_link(link: &Path, target: &Path) {
    if symlink_dir(target, link).is_ok() {
        return;
    }
    let powershell = PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"))
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let status = Command::new(powershell).args([
        "-NoProfile", "-NonInteractive", "-Command",
        "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:VIBEMUX_PIN_TEST_LINK -Target $env:VIBEMUX_PIN_TEST_TARGET | Out-Null",
    ]).env("VIBEMUX_PIN_TEST_LINK", link).env("VIBEMUX_PIN_TEST_TARGET", target)
        .creation_flags(0x0800_0000).status().expect("owned junction fixture");
    assert!(
        status.success(),
        "junction fixture must be created for a meaningful test"
    );
}

#[test]
fn raw_reparse_ancestors_are_refused_before_canonicalization() {
    let fixture = Fixture::new();
    let link = fixture._directory.path().join("linked_tools");
    create_directory_link(&link, &fixture.bin);
    let result = CleanupExecutablePin::acquire(&link.join("CleanupTool.EXE"), &fixture.project);
    assert!(
        result.is_err(),
        "raw reparse ancestor must not be canonicalized away"
    );
    assert!(fs::symlink_metadata(&link).is_ok());
    fs::remove_dir(&link).expect("remove only owned link");
    create_directory_link(&link, &fixture.project);
    assert!(CleanupExecutablePin::acquire(&fixture.executable, &link).is_err());
    fs::remove_dir(&link).expect("remove only owned project link");
    fixture.assert_intact();
}

#[test]
fn a_project_name_prefix_does_not_make_an_unrelated_directory_contained() {
    let fixture = Fixture::new();
    let sibling = fixture._directory.path().join("project-extra");
    fs::create_dir(&sibling).expect("owned sibling");
    let executable = sibling.join("tool.exe");
    fs::write(&executable, EXECUTABLE_BYTES).expect("owned sibling executable");
    let pin =
        CleanupExecutablePin::acquire(&executable, &fixture.project).expect("outside project");
    pin.verify().expect("stable outside pin");
    fixture.assert_intact();
}

#[test]
fn contained_and_invalid_paths_never_create_a_pin() {
    let fixture = Fixture::new();
    let contained = fixture.project.join("inside.exe");
    fs::write(&contained, EXECUTABLE_BYTES).expect("project file");
    let project_alias = PathBuf::from(fixture.project.to_string_lossy().to_ascii_uppercase());
    assert!(CleanupExecutablePin::acquire(&contained, &project_alias).is_err());
    let empty = fixture.bin.join("empty.exe");
    fs::write(&empty, b"").expect("empty fixture");
    for path in [
        PathBuf::from("relative.exe"),
        PathBuf::from("C:relative.exe"),
        PathBuf::from("\\root_relative.exe"),
        PathBuf::from("\\\\host\\share\\foreign.exe"),
        PathBuf::from("\\\\?\\UNC\\host\\share\\foreign.exe"),
        PathBuf::from("\\\\.\\PhysicalDrive0"),
        fixture.bin.join("../bin/CleanupTool.EXE"),
        fixture.bin.join("missing.exe"),
        fixture.executable.with_file_name("CleanupTool.EXE:stream"),
        empty,
        fixture.bin.clone(),
    ] {
        assert!(CleanupExecutablePin::acquire(&path, &fixture.project).is_err());
    }
    assert!(
        CleanupExecutablePin::acquire(&fixture.executable, Path::new("relative_project")).is_err()
    );
    assert!(CleanupExecutablePin::acquire(&fixture.executable, &fixture.executable).is_err());
    fixture.assert_intact();
}

#[test]
fn component_capacity_is_checked_without_opening_an_unbounded_chain() {
    let mut path = PathBuf::from("C:\\");
    for _ in 0..MAX_DIRECTORY_PINS + 1 {
        path.push("component");
    }
    assert!(directory_paths(&path).is_err());
}

#[test]
fn debug_output_redacts_paths_and_object_identity() {
    let fixture = Fixture::new();
    let pin = CleanupExecutablePin::acquire(&fixture.executable, &fixture.project).expect("pin");
    let output = format!("{pin:?}");
    assert_eq!(output, "CleanupExecutablePin(<redacted>)");
    assert!(!output.contains("CleanupTool"));
    fixture.assert_intact();
}
