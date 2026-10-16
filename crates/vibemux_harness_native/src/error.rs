use std::{error, fmt};

/// Public formatting contains codes only; source errors remain inspectable locally.
pub struct Error {
    code: &'static str,
    source: Option<Box<dyn error::Error + Send + Sync>>,
}

impl Error {
    pub fn new(code: &'static str) -> Self {
        Self { code, source: None }
    }

    pub fn with_source(
        code: &'static str,
        source: impl error::Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            code,
            source: Some(Box::new(source)),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code)
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Error")
            .field("code", &self.code)
            .finish_non_exhaustive()
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        self.source.as_deref().map(|source| source as _)
    }
}
