/// Errors produced by the gel core engine
#[derive(Debug, thiserror::Error)]
pub enum GelError {
    /// A package backend reported a failure
    #[error("package backend error: {0}")]
    Backend(String),
    /// An underlying I/O operation failed
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// (De)serialization of state failed
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
}
