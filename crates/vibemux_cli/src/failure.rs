//! Public error codes are independent of private paths and process diagnostics.
use std::{error, fmt};

pub struct Failure {
    code: String,
    message: Option<String>,
    source: Option<Box<dyn error::Error + Send + Sync>>,
}

impl Failure {
    pub fn new(code: &str) -> Self {
        let code = if !code.is_empty()
            && code.len() <= 96
            && code
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            code.to_string()
        } else {
            "cli_operation_failed".to_string()
        };
        Self {
            code,
            message: None,
            source: None,
        }
    }

    pub fn with_source(code: &str, source: impl error::Error + Send + Sync + 'static) -> Self {
        let mut failure = Self::new(code);
        failure.source = Some(Box::new(source));
        failure
    }

    /// A failure that also carries the daemon-backed control verbs' actionable
    /// message. Those messages name only the remediation, never prompts or secrets.
    pub fn with_message(
        code: &str,
        message: String,
        source: impl error::Error + Send + Sync + 'static,
    ) -> Self {
        let mut failure = Self::with_source(code, source);
        failure.message = Some(message);
        failure
    }

    /// A failure the CLI detects itself, with a fixed actionable message and
    /// no underlying error. The message names only the remediation.
    pub fn actionable(code: &str, message: &'static str) -> Self {
        let mut failure = Self::new(code);
        failure.message = Some(message.to_string());
        failure
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.code)
    }
}
impl fmt::Debug for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Failure")
            .field("code", &self.code)
            .finish_non_exhaustive()
    }
}
impl error::Error for Failure {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        self.source.as_deref().map(|source| source as _)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;

    const PRIVATE_PATH: &str = r"C:\Users\private_user\project\.vibemux\control.json";

    fn private_source() -> std::io::Error {
        std::io::Error::other(format!("cannot open {PRIVATE_PATH}"))
    }

    #[test]
    fn only_a_short_snake_case_code_is_kept() {
        let longest = "a".repeat(96);
        for kept in [
            "workspace_unavailable",
            "ui_terminal_required",
            "e2",
            longest.as_str(),
        ] {
            assert_eq!(Failure::new(kept).code(), kept);
        }
        let too_long = "a".repeat(97);
        for replaced in [
            "",
            too_long.as_str(),
            "Workspace_unavailable",
            "workspace-unavailable",
            "workspace unavailable",
            "workspace_unavailable\n",
            PRIVATE_PATH,
            "\u{e9}t\u{e9}",
        ] {
            assert_eq!(
                Failure::new(replaced).code(),
                "cli_operation_failed",
                "{replaced:?}"
            );
            assert_eq!(
                Failure::with_source(replaced, private_source()).code(),
                "cli_operation_failed"
            );
        }
    }

    #[test]
    fn display_and_debug_show_the_code_and_never_the_source_or_message() {
        let failures = [
            Failure::with_source("workspace_unavailable", private_source()),
            Failure::with_message(
                "workspace_unavailable",
                "start the daemon first".to_string(),
                private_source(),
            ),
            Failure::with_source(PRIVATE_PATH, private_source()),
        ];
        for failure in &failures {
            let display = failure.to_string();
            let debug = format!("{failure:?}");
            assert_eq!(display, failure.code());
            assert_eq!(
                debug,
                format!("Failure {{ code: {:?}, .. }}", failure.code())
            );
            for shown in [&display, &debug] {
                assert!(!shown.contains("private_user"), "{shown}");
                assert!(!shown.contains("start the daemon"), "{shown}");
            }
        }
    }

    #[test]
    fn the_message_and_source_stay_available_to_their_own_readers() {
        let plain = Failure::new("ui_terminal_required");
        assert_eq!((plain.message(), plain.source().is_none()), (None, true));
        let chained = Failure::with_source("workspace_unavailable", private_source());
        assert_eq!(chained.message(), None);
        assert!(
            chained
                .source()
                .is_some_and(|source| source.to_string().contains("private_user"))
        );
        let actionable = Failure::with_message(
            "workspace_unavailable",
            "start the daemon first".to_string(),
            private_source(),
        );
        assert_eq!(actionable.message(), Some("start the daemon first"));
        assert!(actionable.source().is_some());
        let detected = Failure::actionable("workspace_git_too_old", "upgrade Git");
        assert_eq!(detected.code(), "workspace_git_too_old");
        assert_eq!(detected.message(), Some("upgrade Git"));
        assert!(detected.source().is_none());
        assert_eq!(detected.to_string(), "workspace_git_too_old");
    }
}
