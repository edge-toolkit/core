//! Binary entrypoint. Pure env-var config — no CLI flags.
//!
//!   PYO3_AGENT_MODULE       (required) — Python module name to import
//!   PYO3_AGENT_PYTHONPATH   (optional) — colon-separated paths prepended
//!                                        to sys.path before the import
//!   WS_SERVER_URL           (optional) — defaults to ws://127.0.0.1:8080/ws
//!   PYO3_AGENT_ID           (optional) — request this agent_id on connect

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use et_ws_pyo3_runner::agent::{AgentConfig, run as run_agent};
use et_ws_pyo3_runner::python::Dispatcher;
use tracing::info;

fn parse_pythonpath(raw: &str) -> Vec<PathBuf> {
    raw.split(':')
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect()
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let module_name = std::env::var("PYO3_AGENT_MODULE")
        .map_err(|_| anyhow!("PYO3_AGENT_MODULE not set; see services/ws-pyo3-runner/python/echo.py"))?;
    let python_path = std::env::var("PYO3_AGENT_PYTHONPATH")
        .map(|s| parse_pythonpath(&s))
        .unwrap_or_default();
    let ws_url = std::env::var("WS_SERVER_URL").unwrap_or_else(|_| {
        format!(
            "ws://localhost:{}/ws",
            edge_toolkit::ports::Services::InsecureWebSocketServer.port()
        )
    });
    let requested_agent_id = std::env::var("PYO3_AGENT_ID").ok().filter(|s| !s.is_empty());

    info!("module={module_name} python_path={python_path:?} ws_url={ws_url}");

    let dispatcher = Dispatcher::import(&module_name, &python_path)
        .with_context(|| format!("import python module `{module_name}`"))?;

    let config = AgentConfig {
        ws_url,
        requested_agent_id,
        dispatcher,
    };

    tokio::select! {
        result = run_agent(config) => result,
        _ = tokio::signal::ctrl_c() => {
            info!("interrupted; shutting down");
            Ok(())
        }
    }
}
