use std::time::Duration;

use et_ws_web_runner::run_module;
use tracing::info;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let module_name = std::env::var("RUNNER_MODULE").or(Err("RUNNER_MODULE not set"))?;
    let ws_url = std::env::var("WS_SERVER_URL").unwrap_or_else(|_| {
        format!(
            "ws://localhost:{}/ws",
            edge_toolkit::ports::Services::InsecureWebSocketServer.port()
        )
    });
    let timeout_secs: u64 = std::env::var("RUNNER_TIMEOUT_SECS")
        .ok()
        .and_then(|val| val.parse().ok())
        .unwrap_or(120);

    info!("et-ws-web-runner: module={module_name} server={ws_url} timeout={timeout_secs}s");

    let result = tokio::time::timeout(Duration::from_secs(timeout_secs), run_module(&module_name, &ws_url)).await;

    match result {
        Ok(Ok(())) => {
            info!("module {module_name} completed successfully");
            Ok(())
        }
        Ok(Err(e)) => Err(e.into()),
        Err(_) => Err(format!("module {module_name} timed out after {timeout_secs}s").into()),
    }
}
