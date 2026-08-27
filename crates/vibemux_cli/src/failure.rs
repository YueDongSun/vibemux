//! Public error codes are independent of private paths and process diagnostics.
use std::{error, fmt};

pub struct Failure {
    code: String,
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
        Self { code, source: None }
    }

    pub fn with_source(code: &str, source: impl error::Error + Send + Sync + 'static) -> Self {
        let mut failure = Self::new(code);
        failure.source = Some(Box::new(source));
        failure
    }

    pub fn code(&self) -> &str {
        &self.code
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
