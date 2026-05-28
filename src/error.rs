//! Error type for all conversion operations.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, ConvertError>;

#[derive(Debug, Error)]
pub enum ConvertError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[cfg(feature = "opencode")]
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("parse error: {0}")]
    Parse(String),

    #[error("unsupported operation: {0}")]
    Unsupported(String),

    #[error("validation: {0}")]
    Validation(String),

    #[error("missing field: {0}")]
    MissingField(&'static str),

    #[error("cancelled: {0}")]
    Cancelled(String),

    #[error("{0}")]
    Other(String),
}

impl From<&str> for ConvertError {
    fn from(s: &str) -> Self {
        ConvertError::Other(s.to_string())
    }
}
impl From<String> for ConvertError {
    fn from(s: String) -> Self {
        ConvertError::Other(s)
    }
}
