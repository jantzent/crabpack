use std::io;

use thiserror::Error;

pub type Result<T, E = CrabpackError> = std::result::Result<T, E>;

/// Error type for crabpack operations.
#[derive(Debug, Error)]
pub enum CrabpackError {
    /// Errors that should be surfaced directly to the end user (similar to
    /// ``VenvPackException`` in the Python implementation).
    #[error("{0}")]
    Message(String),

    /// Generic I/O errors that bubble up from filesystem operations.
    #[error(transparent)]
    Io(#[from] io::Error),

    /// Errors encountered while traversing the filesystem.
    #[error(transparent)]
    WalkDir(#[from] walkdir::Error),

    /// Errors generated from the zip writer implementation.
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
}

impl CrabpackError {
    /// Convenience helper for constructing a user-facing error message.
    pub fn user<T: Into<String>>(msg: T) -> Self {
        CrabpackError::Message(msg.into())
    }
}
