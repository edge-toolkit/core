//! Error type for the runner's connect/register/drive path.

use thiserror::Error;

use crate::python::PythonError;

/// Failure modes of `agent::{initialize, run}`.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RunnerError {
    /// Could not derive the storage HTTP base from the ws-server URL.
    #[error(transparent)]
    Bootstrap(#[from] et_ws_runner_common::BootstrapError),

    /// Importing or initialising the user's Python module failed.
    #[error(transparent)]
    Python(#[from] PythonError),

    /// Connecting to and registering with the ws-server failed.
    #[error(transparent)]
    Connect(#[from] et_ws_runner_common::ConnectError),

    /// A WebSocket send / receive failed while driving the connection.
    #[error("websocket: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),

    /// The dedicated Python dispatch thread could not be spawned.
    #[error("failed to spawn the Python dispatch thread: {0}")]
    WorkerSpawn(#[from] std::io::Error),
}
