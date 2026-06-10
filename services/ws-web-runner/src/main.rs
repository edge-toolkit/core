use et_ws_web_runner::config::Config;
use et_ws_web_runner::run_module;
use tracing::info;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let config = serde_env::from_env::<Config>()?;
    let module = &config.runner.module;
    let ws_url = &config.ws.server_url;

    let run = run_module(module, ws_url);
    let result = if let Some(timeout) = config.runner.timeout {
        info!("et-ws-web-runner: module={module} server={ws_url} timeout={timeout:?}");
        match tokio::time::timeout(timeout, run).await {
            Ok(result) => result,
            Err(_) => return Err(format!("module {module} timed out after {timeout:?}").into()),
        }
    } else {
        info!("et-ws-web-runner: module={module} server={ws_url} timeout=none");
        run.await
    };

    result?;
    info!("module {module} completed successfully");
    Ok(())
}
