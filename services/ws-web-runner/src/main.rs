use et_ws_web_runner::config::Config;
use et_ws_web_runner::run_module;
use tracing::info;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    // Inject V8 flags from RUNNER_V8_FLAGS before any JsRuntime touches V8 (set_flags_from_string is a no-op
    // once V8 has initialised). Used to bisect the gnullvm WASM crash in dotnet-data1 -- e.g. `--no-liftoff`
    // (TurboFan only), `--liftoff-only`, or `--jitless` -- by selecting which WASM compile tier runs.
    let v8_flags = std::env::var("RUNNER_V8_FLAGS").unwrap_or_default();
    if !v8_flags.is_empty() {
        deno_core::v8::V8::set_flags_from_string(&v8_flags);
        info!("applied RUNNER_V8_FLAGS: {v8_flags}");
    }

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
