//! Bounded identifier and path-pattern newtypes shared by every contract.
//!
//! Identifiers are lowercase snake_case so they are safe in file names,
//! event payloads, and rendered prompts. Path patterns are repository
//! relative, forward-slash separated, and contain no traversal; the only
//! wildcard is a trailing `/**` that matches everything below a directory.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use thiserror::Error;

pub const MAX_IDENTIFIER_BYTES: usize = 64;
pub const MAX_PATH_PATTERN_BYTES: usize = 240;
const RECURSIVE_SUFFIX: &str = "/**";

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum IdentifierError {
    #[error("identifier must be 1..=64 bytes of lowercase snake_case")]
    InvalidIdentifier,
    #[error("path pattern must be a bounded relative path without traversal")]
    InvalidPathPattern,
}

impl IdentifierError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidIdentifier => "workflow_invalid_identifier",
            Self::InvalidPathPattern => "workflow_invalid_path_pattern",
        }
    }
}

/// A lowercase snake_case identifier: `[a-z][a-z0-9_]*`, at most 64 bytes,
/// no doubled or trailing underscore.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SpecIdentifier(String);

impl SpecIdentifier {
    pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        if is_snake_case(&value) {
            Ok(Self(value))
        } else {
            Err(IdentifierError::InvalidIdentifier)
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_snake_case(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_IDENTIFIER_BYTES
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_')
        && !value.contains("__")
        && !value.ends_with('_')
}

impl fmt::Debug for SpecIdentifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "SpecIdentifier({})", self.0)
    }
}

impl fmt::Display for SpecIdentifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for SpecIdentifier {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SpecIdentifier {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// A repository-relative path or a directory followed by `/**`.
///
/// Segments are non-empty, never `.` or `..`, and use only ASCII letters,
/// digits, `.`, `_`, and `-` (product paths are external names and may
/// contain hyphens). Comparison is exact and case-sensitive; Windows
/// case-insensitive aliases are rejected by the snapshot scope check, which
/// compares lowercase forms (see [`crate::snapshot`]).
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PathPattern(String);

impl PathPattern {
    pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        if value.len() > MAX_PATH_PATTERN_BYTES {
            return Err(IdentifierError::InvalidPathPattern);
        }
        let base = value.strip_suffix(RECURSIVE_SUFFIX).unwrap_or(&value);
        if !is_relative_path(base) {
            return Err(IdentifierError::InvalidPathPattern);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn is_recursive(&self) -> bool {
        self.0.ends_with(RECURSIVE_SUFFIX)
    }

    /// The literal path, or the directory a recursive pattern covers.
    #[must_use]
    pub fn base(&self) -> &str {
        self.0.strip_suffix(RECURSIVE_SUFFIX).unwrap_or(&self.0)
    }

    /// Whether `path` (a validated relative file path) is covered.
    #[must_use]
    pub fn matches(&self, path: &str) -> bool {
        if self.is_recursive() {
            let base = self.base();
            path.len() > base.len() && path.starts_with(base) && path.as_bytes()[base.len()] == b'/'
        } else {
            path == self.0
        }
    }

    /// Whether every path this pattern covers is also covered by `outer`.
    #[must_use]
    pub fn is_within(&self, outer: &Self) -> bool {
        if outer.is_recursive() {
            outer.matches(self.base()) || (self.is_recursive() && self.base() == outer.base())
        } else {
            !self.is_recursive() && self.0 == outer.0
        }
    }

    /// Whether the two patterns can cover a common path.
    #[must_use]
    pub fn overlaps(&self, other: &Self) -> bool {
        self.is_within(other)
            || other.is_within(self)
            || (self.is_recursive() && self.matches(other.base()))
            || (other.is_recursive() && other.matches(self.base()))
    }
}

/// Whether `value` is a non-empty relative path whose segments are plain
/// names (no traversal, no drive, no backslash, no empty segment).
#[must_use]
pub fn is_relative_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_PATH_PATTERN_BYTES
        && value.split('/').all(|segment| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
}

impl fmt::Debug for PathPattern {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "PathPattern({})", self.0)
    }
}

impl fmt::Display for PathPattern {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for PathPattern {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for PathPattern {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(value: &str) -> PathPattern {
        PathPattern::new(value).expect("valid pattern")
    }

    #[test]
    fn identifiers_accept_only_bounded_snake_case() {
        for valid in ["r01", "track_a", "worker_store", "a"] {
            assert!(SpecIdentifier::new(valid).is_ok(), "{valid}");
        }
        for invalid in [
            "",
            "Track",
            "track-a",
            "track__a",
            "track_",
            "1track",
            "trä",
            &"a".repeat(65),
        ] {
            assert_eq!(
                SpecIdentifier::new(invalid),
                Err(IdentifierError::InvalidIdentifier),
                "{invalid}"
            );
        }
    }

    #[test]
    fn path_patterns_reject_traversal_absolute_and_windows_forms() {
        for invalid in [
            "",
            "/src/server.mjs",
            "../src",
            "src/../server.mjs",
            "src//server.mjs",
            "src\\server.mjs",
            "C:/src",
            "src/./a",
            "**",
            "src/*.mjs",
            "src/**/x",
        ] {
            assert!(PathPattern::new(invalid).is_err(), "{invalid}");
        }
        for valid in [
            "src/server.mjs",
            "tests/worker_a/**",
            "package.json",
            "a-b/c.d",
        ] {
            assert!(PathPattern::new(valid).is_ok(), "{valid}");
        }
    }

    #[test]
    fn recursive_patterns_match_only_below_their_directory() {
        let tests = pattern("tests/worker_a/**");
        assert!(tests.matches("tests/worker_a/x.test.mjs"));
        assert!(tests.matches("tests/worker_a/deep/x.mjs"));
        assert!(!tests.matches("tests/worker_a"));
        assert!(!tests.matches("tests/worker_ab/x.mjs"));
        assert!(!tests.matches("tests/worker_b/x.mjs"));
        let file = pattern("src/server.mjs");
        assert!(file.matches("src/server.mjs"));
        assert!(!file.matches("src/server.mjs.bak"));
    }

    #[test]
    fn containment_and_overlap_are_prefix_trap_safe() {
        assert!(pattern("src/server.mjs").is_within(&pattern("src/**")));
        assert!(pattern("src/a/**").is_within(&pattern("src/**")));
        assert!(!pattern("src/**").is_within(&pattern("src/a/**")));
        assert!(!pattern("srcx/a.mjs").is_within(&pattern("src/**")));
        assert!(pattern("src/**").overlaps(&pattern("src/store.mjs")));
        assert!(!pattern("public/**").overlaps(&pattern("src/**")));
        assert!(!pattern("tests/worker_a/**").overlaps(&pattern("tests/worker_ab/**")));
    }
}
