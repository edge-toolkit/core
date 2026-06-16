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
    let run = run_module(module, ws_url, config.ws.connect_ack_timeout);
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
    // macOS test-only fast exit. ORT 1.22's threadpool unwind races libc++ at
    // process exit on macOS, with this exact stderr at the crash site:
    //
    //   libc++abi: terminating due to uncaught exception of type
    //   std::__1::system_error: mutex lock failed: Invalid argument
    //
    // The runner subprocess then returns exit code None and the integration
    // test's assert fires:
    //
    //   et-ws-wasi-graphics-info exited with code None
    //
    // `libc::_exit` not `std::process::exit`: the Rust one calls libc `exit(3)`,
    // which runs atexit handlers -- that's exactly the path that runs ORT's
    // C++ static destructors and races libc++, so the crash still fires. POSIX
    // `_exit(2)` skips atexit entirely. Useful work has flushed by here (OTLP
    // above, tokio I/O drained when `run_module` returned), so skipping static
    // destructors loses nothing -- but it IS a sharp tool, so it's gated to
    // (a) macOS only (Linux/Windows don't see the crash) and (b) the
    // integration test opting in via ET_WS_WASI_RUNNER_FAST_EXIT (production
    // binary use still does normal teardown).
    #[cfg(target_os = "macos")]
    if std::env::var_os("ET_WS_WASI_RUNNER_FAST_EXIT").is_some() {
        // SAFETY: `_exit(0)` is async-signal-safe and has no preconditions; it
        // terminates the process immediately without running atexit handlers.
        #[expect(
            unsafe_code,
            reason = "libc::_exit is the only way to skip atexit handlers; see comment above"
        )]
        unsafe {
            libc::_exit(0);
        }
    }
    Ok(())
}
