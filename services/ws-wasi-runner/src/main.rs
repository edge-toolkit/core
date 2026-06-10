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

    let module = &config.runner_module;
    let ws_url = &config.ws_server_url;
    info!("et-ws-wasi-runner: module={module} server={ws_url}");
    let result = run_module(module, ws_url).await;

    // Flush before exit so the mock OTLP collector sees the spans we emitted
    // -- `BatchExporter` would otherwise drop the tail when the process exits.
    if let Some(handles) = otel_handles {
        handles.shutdown();
    }
    result?;
    info!("module {module} completed successfully");
    Ok(())
}
