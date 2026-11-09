use thiserror::Error;

pub type Result<T> = std::result::Result<T, SyncError>;

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("{path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("encoding: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Core(#[from] terrarium_core::Error),
    #[error("key: {0}")]
    Key(String),
    #[error("recovery phrase is not valid: {0}")]
    Phrase(String),
    #[error("batch from {device} #{seq} rejected: {reason}")]
    Rejected {
        device: String,
        seq: u64,
        reason: String,
    },
    #[error("transport: {0}")]
    Transport(String),
    #[error("pairing failed: {0}")]
    Pairing(String),
}

impl SyncError {
    pub(crate) fn io(path: &std::path::Path, source: std::io::Error) -> Self {
        SyncError::Io {
            path: path.display().to_string(),
            source,
        }
    }
}
