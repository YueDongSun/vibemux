//! Which Git releases the inventory supports, and how each one keeps the
//! inventory from fetching objects over the network. Pure logic: the caller
//! runs Git and passes its output in.
use crate::failure::Failure;

/// The oldest Git with `git worktree list --porcelain -z`.
const MINIMUM_VERSION: Version = Version::new(2, 36);
/// The oldest Git with the global `--no-lazy-fetch` option.
const NO_LAZY_FETCH_VERSION: Version = Version::new(2, 45);
const MAX_VERSION_DIGITS: usize = 9;

const UNRECOGNIZED_MESSAGE: &str = "Git reported a version this build cannot read. Install a Git release, 2.36 or newer, and retry.";
const MINIMUM_MESSAGE: &str =
    "Git 2.36 or newer is required to read the workspace inventory. Upgrade Git and retry.";
const PROMISOR_MESSAGE: &str = "This repository has a promisor remote (partial clone). Git 2.45 or newer is required to read it without network access. Upgrade Git and retry.";

/// A Git release's major and minor numbers. No option the inventory uses
/// depends on the patch level.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Version {
    major: u32,
    minor: u32,
}

impl Version {
    const fn new(major: u32, minor: u32) -> Self {
        Self { major, minor }
    }

    pub fn has_no_lazy_fetch(self) -> bool {
        self >= NO_LAZY_FETCH_VERSION
    }
}

/// Reads `git version` output, such as `git version 2.43.0`,
/// `git version 2.51.1.windows.1`, or `git version 2.39.5 (Apple Git-154)`,
/// and refuses a release older than `MINIMUM_VERSION`.
pub fn supported_version(reported: &str) -> Result<Version, Failure> {
    let version = parse_version(reported).ok_or_else(|| {
        Failure::actionable("workspace_git_version_unrecognized", UNRECOGNIZED_MESSAGE)
    })?;
    if version < MINIMUM_VERSION {
        return Err(Failure::actionable(
            "workspace_git_too_old",
            MINIMUM_MESSAGE,
        ));
    }
    Ok(version)
}

/// Refuses a Git without `--no-lazy-fetch` when any configuration key can
/// give the repository a promisor remote. Without one, Git has nowhere to
/// fetch a missing object from, so the inventory stays local without the
/// option. Only key names are read, so a `promisor = false` entry also
/// refuses: the check errs toward no network access.
pub fn require_local_objects<'a>(
    version: Version,
    config_keys: impl IntoIterator<Item = &'a str>,
) -> Result<(), Failure> {
    if version.has_no_lazy_fetch() || !config_keys.into_iter().any(names_promisor_remote) {
        return Ok(());
    }
    Err(Failure::actionable(
        "workspace_git_too_old",
        PROMISOR_MESSAGE,
    ))
}

fn parse_version(reported: &str) -> Option<Version> {
    let release = reported.lines().next()?.strip_prefix("git version ")?;
    let mut components = release.split('.');
    let major = version_number(components.next()?)?;
    let minor = version_number(components.next()?)?;
    Some(Version::new(major, minor))
}

fn version_number(component: &str) -> Option<u32> {
    if component.is_empty()
        || component.len() > MAX_VERSION_DIGITS
        || !component.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    component.parse().ok()
}

/// Git creates a promisor remote from `extensions.partialClone`,
/// `remote.<name>.promisor`, and `remote.<name>.partialCloneFilter`
/// (`promisor-remote.c`, unchanged from 2.36 to 2.44). Git lowercases section
/// and variable names; the comparison ignores case anyway.
fn names_promisor_remote(key: &str) -> bool {
    if key.eq_ignore_ascii_case("extensions.partialclone") {
        return true;
    }
    let Some((section, variable)) = key.rsplit_once('.') else {
        return false;
    };
    // A section name has no dot; a remote's subsection name may.
    let in_remote = section
        .split('.')
        .next()
        .is_some_and(|name| name.eq_ignore_ascii_case("remote"));
    in_remote
        && (variable.eq_ignore_ascii_case("promisor")
            || variable.eq_ignore_ascii_case("partialclonefilter"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD: Version = Version::new(2, 43);

    fn refusal(result: Result<impl std::fmt::Debug, Failure>) -> (String, String) {
        let failure = result.unwrap_err();
        let message = failure.message().unwrap_or_default().to_string();
        (failure.code().to_string(), message)
    }

    /// Text output shows a message through `safe_text`, which truncates.
    #[test]
    fn every_message_is_shown_whole_in_text_output() {
        for message in [UNRECOGNIZED_MESSAGE, MINIMUM_MESSAGE, PROMISOR_MESSAGE] {
            assert_eq!(vibemux_frontend::dashboard::safe_text(message), message);
        }
    }

    #[test]
    fn release_and_vendor_version_strings_parse_to_major_and_minor() {
        for (reported, major, minor) in [
            ("git version 2.43.0\n", 2, 43),
            ("git version 2.51.1.windows.1\n", 2, 51),
            ("git version 2.39.5 (Apple Git-154)\n", 2, 39),
            ("git version 2.45.0.rc1\n", 2, 45),
            ("git version 2.45.GIT\n", 2, 45),
            ("git version 2.44\r\n", 2, 44),
            ("git version 3.0.0\nsecond line\n", 3, 0),
        ] {
            let expected = Version::new(major, minor);
            assert_eq!(parse_version(reported), Some(expected), "{reported:?}");
        }
    }

    #[test]
    fn unreadable_version_strings_are_refused() {
        for reported in [
            "",
            "\ngit version 2.43.0",
            "git 2.43.0",
            "git version",
            "git version 2",
            "git version .43.0",
            "git version 2..0",
            "git version +2.43.0",
            "git version 2.4x.0",
            "git version 2.43rc0",
            "git version 1234567890.1.0",
            " git version 2.43.0",
            "Git version 2.43.0",
        ] {
            assert_eq!(parse_version(reported), None, "{reported:?}");
            let (code, message) = refusal(supported_version(reported));
            assert_eq!(code, "workspace_git_version_unrecognized");
            assert!(message.contains("2.36"), "{message}");
        }
    }

    #[test]
    fn versions_compare_by_major_then_minor() {
        assert!(Version::new(2, 9) < MINIMUM_VERSION);
        assert!(Version::new(3, 0) > NO_LAZY_FETCH_VERSION);
        assert!(!Version::new(2, 44).has_no_lazy_fetch());
        assert!(NO_LAZY_FETCH_VERSION.has_no_lazy_fetch());
        assert!(Version::new(3, 0).has_no_lazy_fetch());
    }

    #[test]
    fn git_older_than_the_minimum_is_refused_with_a_remedy() {
        let (code, message) = refusal(supported_version("git version 2.35.8\n"));
        assert_eq!(code, "workspace_git_too_old");
        assert!(message.contains("2.36"), "{message}");
        let minimum = supported_version("git version 2.36.0\n").unwrap();
        assert_eq!(minimum, MINIMUM_VERSION);
        assert_eq!(supported_version("git version 2.43.0\n").unwrap(), OLD);
    }

    #[test]
    fn old_git_is_refused_only_where_a_promisor_remote_is_configured() {
        let plain = "core.bare\0remote.origin.url\0remote.origin.fetch\0filter.lfs.clean\0";
        assert!(require_local_objects(OLD, plain.split('\0')).is_ok());
        for key in [
            "extensions.partialclone",
            "extensions.partialClone",
            "remote.origin.promisor",
            "remote.origin.partialclonefilter",
            "remote.Mirror.Name.partialCloneFilter",
            "remote.promisor",
        ] {
            let keys = format!("{plain}{key}\0");
            let (code, message) = refusal(require_local_objects(OLD, keys.split('\0')));
            assert_eq!(code, "workspace_git_too_old", "{key}");
            assert!(message.contains("2.45"), "{message}");
            let current = require_local_objects(NO_LAZY_FETCH_VERSION, keys.split('\0'));
            assert!(current.is_ok(), "{key}");
        }
        for key in [
            "remote.origin.promisorx",
            "remote.origin.url",
            "remotes.origin.promisor",
            "filter.remote.promisor",
            "extensions.partialclonex",
            "extensions.objectformat",
            "promisor",
        ] {
            assert!(require_local_objects(OLD, [key]).is_ok(), "{key}");
        }
    }
}
