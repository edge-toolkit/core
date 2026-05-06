use anyhow::Context;
use edge_toolkit::config::OtlpConfig;
use et_ws_wasi_runner::run_module;
use serde::Deserialize;
use tracing::info;

/// Tiny envelope so the same `OTLP_*` env vars used by ws-server's `Config`
/// (deserialised via serde-env) work here too.
#[derive(Debug, Default, Deserialize)]
struct EnvConfig {
    otlp: Option<OtlpConfig>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let env_config = serde_env::from_env::<EnvConfig>().unwrap_or_default();

    let otel_handles = if let Some(otlp_config) = &env_config.otlp {
        Some(et_otlp::init(otlp_config))
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
            .init();
        None
    };

    let module_name = std::env::var("RUNNER_MODULE").context("RUNNER_MODULE not set")?;
    let ws_url = std::env::var("WS_SERVER_URL").unwrap_or_else(|_| {
        format!(
            "ws://localhost:{}/ws",
            edge_toolkit::ports::Services::InsecureWebSocketServer.port()
        )
    });

    info!("et-ws-wasi-runner: module={module_name} server={ws_url}");
    let result = run_module(&module_name, &ws_url).await;

    // Flush before exit so the mock OTLP collector sees the spans we emitted
    // — `BatchExporter` would otherwise drop the tail when the process exits.
    if let Some(handles) = otel_handles {
        handles.shutdown();
    }
    result?;
    info!("module {module_name} completed successfully");
    Ok(())
}
