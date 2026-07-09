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

    // Inject V8 flags from V8_FLAGS (config.v8_flags) before any JsRuntime touches V8 --
    // set_flags_from_string is a no-op once V8 has initialised. Used to bisect the gnullvm WASM crash in
    // dotnet-data1 by selecting the WASM compile tier (e.g. --no-liftoff / --liftoff-only / --jitless).
    let v8_flags = config.v8_flags.as_deref().unwrap_or_default();
    if !v8_flags.is_empty() {
        deno_core::v8::V8::set_flags_from_string(v8_flags);
        info!("applied V8_FLAGS: {v8_flags}");
    }

    let run = run_module(module, ws_url, config.et_test_coverage);
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
