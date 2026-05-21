//! Binary entrypoint. Pure env-var config — no CLI flags.
//!
//!   PYO3_AGENT_MODULE       (required) — Python module name to import
//!   PYO3_AGENT_PYTHONPATH   (optional) — colon-separated paths prepended
//!                                        to sys.path before the import
//!   WS_SERVER_URL           (optional) — defaults to ws://127.0.0.1:8080/ws
//!   PYO3_AGENT_ID           (optional) — request this agent_id on connect

use std::path::PathBuf;

use anyhow::{Result, anyhow};
use et_ws_pyo3_runner::agent::{AgentConfig, initialize, run as run_agent};
use tracing::info;

fn parse_pythonpath(raw: &str) -> Vec<PathBuf> {
    raw.split(':')
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect()
}

// Multi-threaded runtime so Python's sync `WsStorage.get/put` (which
// `blocking_recv` on a oneshot reply) doesn't stall the WS loop —
// another worker thread keeps polling the storage task while one
// thread is parked on Python.
#[tokio::main]
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

    let agent = initialize(
        &module_name,
        &python_path,
        AgentConfig {
            ws_url,
            requested_agent_id,
        },
    )?;

    tokio::select! {
        result = run_agent(agent) => result,
        _ = tokio::signal::ctrl_c() => {
            info!("interrupted; shutting down");
            Ok(())
        }
    }
}
