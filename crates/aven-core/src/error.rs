use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorKind {
    Validation,
    NotFound,
    OpenConflict,
    GenerationConflict,
}

#[derive(Debug)]
pub(crate) struct CoreError {
    kind: ErrorKind,
    message: String,
}

impl CoreError {
    pub(crate) fn validation(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Validation, message)
    }

    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::NotFound, message)
    }

    pub(crate) fn open_conflict(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::OpenConflict, message)
    }

    pub(crate) fn generation_conflict(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::GenerationConflict, message)
    }

    pub(crate) const fn kind(&self) -> ErrorKind {
        self.kind
    }

    fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for CoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CoreError {}
