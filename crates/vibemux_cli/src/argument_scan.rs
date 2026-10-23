//! Shared tail scanner for the read-oriented control verbs (`tasks`, `plugins`,
//! `terminal`): every flag takes exactly one value (never another `--` option)
//! and appears at most once, and every other token is a positional. A verb checks its own flag and positional
//! grammar against the scan, so no verb re-implements the loop.

use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

use crate::DaemonCliError;

const PROJECT_ROOT_FLAG: &str = "--project-root";

/// The arguments after a verb's action, split into flags and positionals.
#[derive(Debug)]
pub(crate) struct ScannedTail<'a> {
    pub project_root: Option<PathBuf>,
    flags: BTreeMap<&'a str, &'a str>,
    positionals: Vec<&'a str>,
}

/// Splits `<action> <tail...>`; the action must be present and UTF-8.
pub(crate) fn split_action(arguments: &[OsString]) -> Result<(&str, &[OsString]), DaemonCliError> {
    let (action, rest) = arguments
        .split_first()
        .ok_or(DaemonCliError::InvalidArguments)?;
    let action = action.to_str().ok_or(DaemonCliError::InvalidArguments)?;
    Ok((action, rest))
}

/// The value after the flag at `flag_index`: present, non-empty, and not
/// itself an option, so `--project-root --limit` is a missing value rather than
/// a project named `--limit`.
fn value_after(tail: &[OsString], flag_index: usize) -> Result<&OsString, DaemonCliError> {
    tail.get(flag_index + 1)
        .filter(|value| {
            !value.is_empty() && !value.to_str().is_some_and(|text| text.starts_with("--"))
        })
        .ok_or(DaemonCliError::InvalidArguments)
}

/// Scans `tail` accepting `--project-root` plus the verb's `allowed_flags`.
/// A flag that is repeated, unknown, or missing its value is refused, and a
/// value never starts with `--`.
pub(crate) fn scan_tail<'a>(
    tail: &'a [OsString],
    allowed_flags: &[&str],
) -> Result<ScannedTail<'a>, DaemonCliError> {
    let mut scanned = ScannedTail {
        project_root: None,
        flags: BTreeMap::new(),
        positionals: Vec::new(),
    };
    let mut index = 0;
    while index < tail.len() {
        let token = tail[index]
            .to_str()
            .ok_or(DaemonCliError::InvalidArguments)?;
        if token == PROJECT_ROOT_FLAG {
            if scanned.project_root.is_some() {
                return Err(DaemonCliError::InvalidArguments);
            }
            scanned.project_root = Some(PathBuf::from(value_after(tail, index)?));
            index += 2;
        } else if token.starts_with("--") {
            if !allowed_flags.contains(&token) {
                return Err(DaemonCliError::InvalidArguments);
            }
            let value = value_after(tail, index)?
                .to_str()
                .ok_or(DaemonCliError::InvalidArguments)?;
            if scanned.flags.insert(token, value).is_some() {
                return Err(DaemonCliError::InvalidArguments);
            }
            index += 2;
        } else {
            scanned.positionals.push(token);
            index += 1;
        }
    }
    Ok(scanned)
}

impl<'a> ScannedTail<'a> {
    /// The value of `name`, when the flag was given.
    pub fn flag(&self, name: &str) -> Option<&'a str> {
        self.flags.get(name).copied()
    }

    /// Refuses a positional the action does not take.
    pub fn without_positionals(&self) -> Result<(), DaemonCliError> {
        if self.positionals.is_empty() {
            Ok(())
        } else {
            Err(DaemonCliError::InvalidArguments)
        }
    }

    /// Exactly `N` positionals, in order.
    pub fn positionals<const N: usize>(&self) -> Result<[&'a str; N], DaemonCliError> {
        <[&'a str; N]>::try_from(self.positionals.as_slice())
            .map_err(|_| DaemonCliError::InvalidArguments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn flags_positionals_and_project_root_are_separated() {
        let tail = arguments(&["one", "--limit", "5", "--project-root", "C:\\a b", "two"]);
        let scanned = scan_tail(&tail, &["--limit"]).expect("valid tail");
        assert_eq!(scanned.flag("--limit"), Some("5"));
        assert_eq!(scanned.flag("--after"), None);
        assert_eq!(scanned.project_root, Some(PathBuf::from("C:\\a b")));
        assert_eq!(scanned.positionals::<2>().unwrap(), ["one", "two"]);
        assert!(scanned.positionals::<1>().is_err());
        assert!(scanned.without_positionals().is_err());
    }

    #[test]
    fn an_empty_tail_has_no_flags_or_positionals() {
        let scanned = scan_tail(&[], &[]).expect("empty tail");
        assert!(scanned.project_root.is_none());
        assert!(scanned.without_positionals().is_ok());
        assert_eq!(scanned.positionals::<0>().unwrap(), [] as [&str; 0]);
    }

    #[test]
    fn an_option_is_never_taken_as_a_flags_value() {
        for tail in [
            &["--project-root", "--unknown"][..],
            &["--project-root", "--limit", "5"][..],
            &["--limit", "--project-root", "C:\\a"][..],
            &["--limit", "--limit"][..],
            &["--limit", "--"][..],
        ] {
            assert_eq!(
                scan_tail(&arguments(tail), &["--limit"]).map(|_| ()),
                Err(DaemonCliError::InvalidArguments),
                "{tail:?}"
            );
        }
        // A single dash is an ordinary value, and a path may carry the dashes
        // after a directory part.
        let tail = arguments(&["--limit", "-5", "--project-root", ".\\--odd"]);
        let scanned = scan_tail(&tail, &["--limit"]).expect("ordinary values");
        assert_eq!(scanned.flag("--limit"), Some("-5"));
        assert_eq!(scanned.project_root, Some(PathBuf::from(".\\--odd")));
    }

    #[test]
    fn unknown_repeated_and_valueless_flags_are_refused() {
        for tail in [
            &["--other", "1"][..],
            &["--limit", "1", "--limit", "2"][..],
            &["--limit"][..],
            &["--project-root"][..],
            &["--project-root", ""][..],
            &["--project-root", "a", "--project-root", "b"][..],
        ] {
            assert_eq!(
                scan_tail(&arguments(tail), &["--limit"]).map(|_| ()),
                Err(DaemonCliError::InvalidArguments),
                "{tail:?}"
            );
        }
    }
}
