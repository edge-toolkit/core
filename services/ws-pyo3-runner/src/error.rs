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

    /// A module fetched from the hub was not valid UTF-8, so it is not Python source.
    #[error("module `{module}`: hub served `{file}`, which is not UTF-8")]
    ModuleNotUtf8 {
        /// Published name the module was fetched under.
        module: String,
        /// File the module's `package.json` named as its entry point.
        file: String,
    },
}
