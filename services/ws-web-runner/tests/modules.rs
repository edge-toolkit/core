//! Integration tests that run each browser-targeted module via `et-ws-web-runner`.
//!
//! Starts a ws-server in-process. Mirrors `services/ws-wasi-runner/tests/modules.rs`.
//!
//! ## Modules that cannot run under Deno (documented reasons)
//!
//! ### WASI modules (require wasmtime, not a JS runtime)
//!
//! - **et-ws-wasi-comm1, et-ws-wasi-data1, et-ws-wasi-graphics-info**
//!
//! ### Hardware / browser-only APIs
//!
//! - **et-ws-audio1** -- `navigator.mediaDevices.getUserMedia` (microphone)
//! - **et-ws-video1** -- `navigator.mediaDevices.getUserMedia` (camera)
//! - **et-ws-sensor1** -- `DeviceMotionEvent` / `DeviceOrientationEvent`
//! - **et-ws-speech-recognition** -- `webkitSpeechRecognition`
//! - **et-ws-bluetooth** -- `navigator.bluetooth` (Web Bluetooth API)
//! - **et-ws-nfc** -- `NDEFReader` (Web NFC API, Chrome-on-Android only)
//! - **et-ws-geolocation** -- `navigator.geolocation`
//! - **et-ws-face-detection** -- camera (`getUserMedia`) + ONNX model
//! - **et-ws-har1** -- accelerometer (`DeviceMotionEvent`) + ONNX model
//! - **et-ws-pyface1** -- Pyodide + camera + ONNX model
//!
//! ### Non-JS module loaders / incompatible runtimes
//!
//! All currently-shipped data1-family modules run -- including
//! `et-ws-zig-data1`, which spawns a module `Worker` and proxies
//! WebSocket + REST through a `SharedArrayBuffer` with the worker
//! blocking on `Atomics.wait`. That worked once the runner switched
//! from raw `JsRuntime` to `deno_runtime::MainWorker`, which provides
//! the `CreateWebWorkerCb` callback this runner wires recursively so
//! workers can spawn their own workers, sharing a
//! `CrossIsolateStore<SharedRef<BackingStore>>` across isolates for
//! the `SharedArrayBuffer` transfer.

#![cfg(test)]
#![expect(
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    reason = "test code: process spawn failure or non-zero exit fails the test; pywasm1 skip uses println"
)]

use rstest::rstest;

#[rstest]
#[case::data1("et-ws-data1")]
#[case::pydata1("et-ws-pydata1")]
#[case::graphics_info("et-ws-graphics-info")]
#[case::dotnet_data1("et-ws-dotnet-data1")]
#[case::java_data1("et-ws-java-data1")]
#[case::zig_data1("et-ws-zig-data1")]
#[case::pywasm1("et-ws-pywasm1")]
fn module_runs_successfully(#[case] module: &str) {
    if module == "et-ws-pywasm1" && !pywasm1_pkg_built() {
        println!("skipping {module}: pkg/ not built (run `mise run build-pywasm1-module`)");
        return;
    }
    let server = et_ws_test_server::start();
    run_runner_with_timeout(module, &server.ws_url, 90);
}

/// pywasm1 depends on building `rustpython_wasm` from an external clone
/// (`build-rustpython-wasm`), which the default `build-modules` task skips
/// because of the multi-minute rustpython compile. Skip the test on hosts
/// without the pkg/ output instead of failing -- pinning to it would block
/// every CI lane and dev box that hasn't paid that cost.
#[expect(
    clippy::single_call_fn,
    reason = "distinct probe step; kept named for the skip-trace log line"
)]
fn pywasm1_pkg_built() -> bool {
    edge_toolkit::config::get_project_root()
        .join("services/ws-modules/pywasm1/pkg/package.json")
        .exists()
}

/// Spawn two runners against one ws-server and assert both finish ok.
/// Used by communication modules that need to discover at least one peer
/// via `et-list-agents` before they can complete (comm1, dart-comm1).
#[rstest]
#[case::comm1("et-ws-comm1")]
#[case::dart_comm1("et-ws-dart-comm1")]
fn multi_agent_module(#[case] module: &str) {
    let server = et_ws_test_server::start();

    // Each runner needs its own thread because Command::output() blocks until
    // the child closes its stdout pipe -- a sequential wait would let the
    // second child's pipe fill and stall it before we ever read it. A failed
    // runner panics, surfacing through `thread::join().Err(_)`.
    let handles: Vec<_> = (0_u32..2)
        .map(|index| {
            let url = server.ws_url.clone();
            let module = module.to_owned();
            let label = format!("agent{index}");
            std::thread::spawn(move || {
                run_runner_with_timeout(&module, &url, 180);
                label
            })
        })
        .collect();

    let mut failed: Vec<String> = Vec::new();
    for (index, handle) in handles.into_iter().enumerate() {
        if handle.join().is_err() {
            failed.push(format!("agent{index}"));
        }
    }
    assert!(failed.is_empty(), "{module} failed in: {}", failed.join(", "));
}

/// Spawn one `et-ws-web-runner` against `ws_url` and panic with the
/// captured stdout/stderr on non-zero exit. `timeout_secs` is passed as
/// `RUNNER_TIMEOUT` (humantime, e.g. `120s`); the multi-agent harness bumps
/// it because two cold V8 starts contending for the same box widen the
/// discovery window past the single-agent budget.
fn run_runner_with_timeout(module: &str, ws_url: &str, timeout_secs: u32) {
    let bin = env!("CARGO_BIN_EXE_et-ws-web-runner");
    let output = std::process::Command::new(bin)
        .env("RUNNER_MODULE", module)
        .env("WS_SERVER_URL", ws_url)
        .env("RUNNER_TIMEOUT", format!("{timeout_secs}s"))
        .output()
        .expect("failed to spawn et-ws-web-runner");

    if output.status.success() {
        return;
    }
    let code = output
        .status
        .code()
        .map_or_else(|| "signal".to_string(), |code| code.to_string());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    panic!("{module} failed (code {code})\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}");
}
