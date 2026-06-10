use et_ws_wasi_runner::config::Config;
use et_ws_wasi_runner::run_module;
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = serde_env::from_env::<Config>()?;

    #[expect(
        clippy::option_if_let_else,
        reason = "None branch installs an alternate tracing subscriber as a side effect; map_or_else hides it"
    )]
    let otel_handles = if let Some(otlp_config) = &config.otlp {
        Some(et_otlp::init(otlp_config))
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
            .init();
        None
    };

    let module = &config.runner.module;
    let ws_url = &config.ws.server_url;
    let timeout = config.runner.timeout;
    let run = run_module(module, ws_url);
    // `None` outcome == timed out; `Some(_)` carries the module's own result.
    let outcome = if let Some(limit) = timeout {
        info!("et-ws-wasi-runner: module={module} server={ws_url} timeout={limit:?}");
        tokio::time::timeout(limit, run).await.ok()
    } else {
        info!("et-ws-wasi-runner: module={module} server={ws_url}");
        Some(run.await)
    };

    // Flush before exit so the mock OTLP collector sees the spans we emitted
    // -- `BatchExporter` would otherwise drop the tail when the process exits.
    if let Some(handles) = otel_handles {
        handles.shutdown();
    }

    let Some(result) = outcome else {
        return Err(format!("module {module} timed out after {:?}", timeout.unwrap_or_default()).into());
    };
    result?;
    info!("module {module} completed successfully");
    Ok(())
}
