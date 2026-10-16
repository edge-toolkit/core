//! Bounded, application-neutral recovery supervision, separate from fault injection and the WebSocket hub.

pub mod cli;
pub mod config;
pub mod kubernetes;
pub mod persistence;
pub mod policy;
pub mod runtime;

/// Errors preserve the failed operation instead of treating missing observations as healthy workloads.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RecoveryError {
    /// Invalid configuration, evidence, or command outcome.
    #[error("{0}")]
    Invalid(String),
    /// Filesystem or process creation failed.
    #[error("I/O failure: {0}")]
    Io(#[from] std::io::Error),
    /// A complete state document could not replace its predecessor.
    #[error("state persistence failed: {0}")]
    Persist(#[from] tempfile::PersistError),
    /// A blocking persistence task could not complete.
    #[error("state persistence task failed: {0}")]
    PersistenceTask(#[from] tokio::task::JoinError),
    /// The configured budget is already owned or cannot be locked on this filesystem.
    #[error("cannot acquire budget lock: {0}")]
    Lease(#[from] std::fs::TryLockError),
    /// The external Kubernetes command exceeded its allowed lifetime.
    #[error("kubectl process exceeded its deadline: {0}")]
    CommandTimeout(#[from] tokio::time::error::Elapsed),
    /// A JSON payload could not be decoded or encoded.
    #[error("JSON failure: {0}")]
    Json(#[from] serde_json::Error),
    /// The configuration was not valid TOML.
    #[error("configuration failure: {0}")]
    Toml(#[from] toml::de::Error),
    /// Connecting and registering with the hub failed.
    #[error("hub registration failed: {0}")]
    Connect(#[from] et_ws_runner_common::ConnectError),
    /// A live hub connection failed.
    #[error("hub connection failed: {0}")]
    Socket(Box<tokio_tungstenite::tungstenite::Error>),
}

impl From<tokio_tungstenite::tungstenite::Error> for RecoveryError {
    fn from(error: tokio_tungstenite::tungstenite::Error) -> Self {
        Self::Socket(Box::new(error))
    }
}

impl From<tokio::sync::mpsc::error::TrySendError<(String, edge_toolkit::ws::CloudEvent)>> for RecoveryError {
    fn from(error: tokio::sync::mpsc::error::TrySendError<(String, edge_toolkit::ws::CloudEvent)>) -> Self {
        Self::Invalid(format!("recovery evidence queue unavailable: {error}"))
    }
}
