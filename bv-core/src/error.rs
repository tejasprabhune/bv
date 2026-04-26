use thiserror::Error;

#[derive(Debug, Error)]
pub enum BvError {
    #[error("failed to parse manifest: {0}")]
    ManifestParse(String),

    #[error("failed to parse lockfile: {0}")]
    LockfileParse(String),

    #[error("container runtime '{runtime}' not available: {reason}")]
    RuntimeNotAvailable { runtime: String, reason: String },

    #[error("runtime error: {0}")]
    RuntimeError(String),

    #[error("index error: {0}")]
    IndexError(String),

    #[error("hardware requirements not met: {0}")]
    HardwareMismatch(String),

    #[error("reference data error: {0}")]
    ReferenceDataError(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, BvError>;
