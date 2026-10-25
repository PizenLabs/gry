use thiserror::Error;

#[derive(Debug, Error)]
pub enum GryError {
    #[error("{0}")]
    Message(String),
    #[error("project not found or not indexed: {0}")]
    ProjectNotFound(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("indexing failed: {0}")]
    Indexing(String),
    #[error("storage error: {0}")]
    Storage(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("operation cancelled")]
    Cancelled,
    #[error("project busy: {0}")]
    Busy(String),
}

pub type GryResult<T> = Result<T, GryError>;

impl GryError {
    pub fn msg(message: impl Into<String>) -> Self {
        GryError::Message(message.into())
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        GryError::InvalidArgument(message.into())
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        GryError::Unsupported(message.into())
    }
}
