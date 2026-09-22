use et_ws_web_runner::config::Config;
use et_ws_web_runner::run_module;
use tracing::info;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (config, _telemetry) = et_otlp::load_telemetered::<Config>()?;

    let module = &config.runner.module;
    let ws_url = &config.ws.server_url;

    apply_v8_flags(&config);

    // Coverage capture exists only under the `coverage` feature; there `ET_TEST_COVERAGE` activates it.
    #[cfg(feature = "coverage")]
    let coverage = config.et_test_coverage;
    #[cfg(not(feature = "coverage"))]
    let coverage = false;
    let run = run_module(module, ws_url, coverage);
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

/// Inject `V8_FLAGS` before any `JsRuntime` touches V8.
///
/// The ordering is load-bearing: `set_flags_from_string` is a no-op once V8 has initialised. Used to bisect
/// the gnullvm WASM crash in dotnet-data1 by selecting the WASM compile tier (e.g. `--no-liftoff`,
/// `--liftoff-only`, `--jitless`).
#[expect(
    clippy::single_call_fn,
    reason = "a distinct step that must run before the runtime starts; named so its ordering is legible"
)]
fn apply_v8_flags(config: &Config) {
    let v8_flags = config.v8_flags.as_deref().unwrap_or_default();
    if !v8_flags.is_empty() {
        deno_core::v8::V8::set_flags_from_string(v8_flags);
        info!("applied V8_FLAGS: {v8_flags}");
    }
}
