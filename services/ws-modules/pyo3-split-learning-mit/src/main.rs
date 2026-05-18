//! Binary entrypoint. Sets up tracing, loads model weights if available,
//! then runs the WebSocket agent loop until the server closes or Ctrl-C.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use et_ws_pyo3_split_learning_mit::agent::{AgentConfig, run as run_agent};
use et_ws_pyo3_split_learning_mit::python::{ModelHandle, default_python_path_extras};
use tracing::info;

/// CLI options. Defaults match the demo's `scripts/server.py` so an existing
/// client.py invocation lights up against this agent without changes.
#[derive(Debug, Parser)]
#[command(
    name = "et-ws-pyo3-split-learning-mit",
    about = "Embedded-PyTorch split-learning agent for et-ws-server"
)]
struct Cli {
    /// Websocket URL to connect to (et-ws-server's `/ws`).
    #[arg(long, env = "WS_SERVER_URL", default_value = "ws://127.0.0.1:8080/ws")]
    ws_url: String,

    /// ONNX file to load/save server-side model weights. If the file exists,
    /// weights are loaded on startup. Trained weights are written back here
    /// on clean shutdown.
    #[arg(long, env = "SERVER_ONNX_PATH")]
    onnx_path: Option<PathBuf>,

    /// SGD learning rate. Matches `server.py --learning-rate` default.
    #[arg(long, default_value_t = 1e-4)]
    learning_rate: f64,

    /// Lightning Fabric accelerator. Forwarded verbatim to `L.Fabric(...)`.
    #[arg(long, default_value = "auto")]
    accelerator: String,

    /// Additional `sys.path` entries to prepend before importing
    /// `split_learning`. Repeat for multiple paths. By default the agent
    /// resolves `split-learning-demo/packages/split-learning-demo/src`
    /// relative to the workspace, or honours `SPLIT_LEARNING_DEMO_SRC`.
    #[arg(long = "python-path")]
    python_path: Vec<PathBuf>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let cli = Cli::parse();

    let mut python_path = cli.python_path.clone();
    if python_path.is_empty() {
        python_path.extend(default_python_path_extras());
    }
    info!("split-learning python_path extras: {python_path:?}");

    let model = ModelHandle::new(
        cli.onnx_path.clone(),
        cli.learning_rate,
        &cli.accelerator,
        &python_path,
    )
    .context("initialise embedded PyTorch model")?;

    let config = AgentConfig {
        ws_url: cli.ws_url.clone(),
        model,
    };

    tokio::select! {
        result = run_agent(config) => result,
        _ = tokio::signal::ctrl_c() => {
            info!("interrupted; shutting down");
            Ok(())
        }
    }
}
