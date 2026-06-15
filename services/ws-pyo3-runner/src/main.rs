//! Binary entrypoint; pure env-var config, no CLI flags.
//!
//! Configuration is deserialised from the environment by
//! [`et_ws_pyo3_runner::config::Config`]; see that module for the full variable
//! list (`RUNNER_MODULE`, `RUNNER_TIMEOUT`, `WS_SERVER_URL`, `PYO3_PYTHONPATH`,
//! `PYO3_AGENT_ID`).

#![expect(
    clippy::integer_division_remainder_used,
    reason = "tokio::select! expands to % internally"
)]

use et_ws_pyo3_runner::agent::{AgentConfig, initialize, run as run_agent};
use et_ws_pyo3_runner::config::Config;
use tracing::info;

// Multi-threaded runtime. Python runs on its own OS thread (see
// `agent::run`'s dispatch worker), and its sync `WsStorage.get/put` park that
// thread on a oneshot reply; the runtime's worker threads keep driving the
// storage task and the WS loop so those replies resolve while Python waits.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let config = serde_env::from_env::<Config>()?;
    let module = config.runner.module.clone();
    let python_path = config.pyo3.python_path();
    let ws_url = config.ws.server_url.clone();
    let requested_agent_id = config.pyo3.agent_id.clone();
    let connect_ack_timeout = config.ws.connect_ack_timeout;

    info!("module={module} python_path={python_path:?} ws_url={ws_url}");

    let agent = initialize(
        &module,
        &python_path,
        AgentConfig {
            ws_url,
            requested_agent_id,
            connect_ack_timeout,
        },
    )?;

    let driven = async {
        tokio::select! {
            result = run_agent(agent) => result,
            _ = tokio::signal::ctrl_c() => {
                info!("interrupted; shutting down");
                Ok(())
            }
        }
    };

    let Some(limit) = config.runner.timeout else {
        driven.await?;
        return Ok(());
    };
    let Ok(result) = tokio::time::timeout(limit, driven).await else {
        info!("run timeout {limit:?} elapsed; shutting down");
        return Ok(());
    };
    result?;
    Ok(())
}
