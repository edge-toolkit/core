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
//! - **et-ws-llm1** -- `navigator.gpu` (WebGPU) + the page's transformers.js loader and chat panel
//! - **et-ws-pyface1** -- Pyodide + camera + ONNX model
//! - **et-ws-pydemo1** -- Pyodide + camera + microphone combined demo
//! - **et-ws-pyeye1** -- Pyodide + camera + `MediaPipe` `FaceLandmarker` (tflite) -> eye boxes
//! - **et-ws-pyspeech1** -- Pyodide + microphone + ONNX model
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
//!
//! ### R modules (webR spawns a *classic* Worker)
//!
//! - **et-ws-rdata1, et-ws-rcomm1** -- both run R via webR, whose
//!   `WebR.init()` spawns its interpreter on a **classic** Web Worker
//!   (`new Worker(url)`, no `{ type: "module" }`). Deno implements only
//!   module workers, so that call throws `NotSupportedError: Classic
//!   workers are not supported` -- the module aborts in its `init()`
//!   default export before any of its own logic (rdata1's httr2
//!   storage round-trip, rcomm1's WebSocket) runs. Unlike the
//!   zig-data1 case above (a *module* worker the runner's recursive
//!   `CreateWebWorkerCb` supports), there is no runner-side fix: the
//!   limitation is Deno's, and webR's classic worker is upstream. These
//!   modules work in a real browser (which supports classic workers +
//!   the `SharedArrayBuffer` channel the ws-server's COOP/COEP headers
//!   enable); `r_module_load_fails` pins the Deno failure so a future
//!   runner that gains classic-worker support is caught and the module
//!   is promoted to `module_runs_successfully`.

#![cfg(test)]

use edge_toolkit::config::{Language, mise_env_includes};
#[cfg(feature = "coverage")]
use fs_err as fs;
use rstest::rstest;

#[rstest]
#[case::data1("et-ws-data1", Language::Rust)]
#[case::except1("et-ws-except1", Language::Rust)]
#[case::pydata1("et-ws-pydata1", Language::Python)]
#[case::graphics_info("et-ws-graphics-info", Language::Rust)]
#[case::dotnet_data1("et-ws-dotnet-data1", Language::Dotnet)]
#[case::java_data1("et-ws-java-data1", Language::Java)]
#[case::zig_data1("et-ws-zig-data1", Language::Zig)]
#[case::zig_except1("et-ws-zig-except1", Language::Zig)]
#[case::dart_data1("et-ws-dart-data1", Language::Dart)]
#[case::kotlin_data1("et-ws-kotlin-data1", Language::Kotlin)]
#[case::pywasm1("et-ws-pywasm1", Language::Python)]
#[case::js_data1("et-ws-js-data1", Language::Js)]
fn module_runs_successfully(#[case] module: &str, #[case] language: Language) {
    // When CI narrows MISE_ENV (e.g. `dotnet,rust`) the env-gated guest
    // configs don't load and the matching `pkg/` never gets built. Skip
    // cases whose language isn't loaded instead of 404'ing on the module
    // fetch.
    if !mise_env_includes(language) {
        println!(
            "skipping {module}: requires the `{}` mise env, not loaded",
            language.as_str()
        );
        return;
    }
    if module == "et-ws-dotnet-data1" && !dotnet_data1_pkg_built() {
        println!("skipping {module}: pkg/ not built (build-ws-dotnet-data1-module has not run on this host)");
        return;
    }
    if module == "et-ws-js-data1" && !js_data1_pkg_built() {
        println!("skipping {module}: pkg/ not built (build-ws-js-data1-module has not run on this host)");
        return;
    }
    if module == "et-ws-kotlin-data1" && !kotlin_data1_pkg_built() {
        println!("skipping {module}: pkg/ not built (build-ws-kotlin-data1-module has not run on this host)");
        return;
    }
    let server = et_ws_test_server::start();
    run_runner_with_timeout(module, &server.ws_url, 90);
    #[cfg(feature = "coverage")]
    collect_module_coverage(&server);
}

/// Run each math1 module through the storage-driven exchange and verify its stored model.
///
/// The fake-agent side lives in `et_ws_test_server::math1`: it injects the canonical input JSON
/// into storage, broadcasts the `math1-input` pointer, and reads back the module's
/// `math1-output.json`, which is verified against the expected weights for that input. The
/// runner's own exit status is asserted too, so a module that errors after storing its output
/// still fails the case.
#[rstest]
#[case::math1("et-ws-math1", Language::Rust)]
#[case::dart_math1("et-ws-dart-math1", Language::Dart)]
#[case::dotnet_math1("et-ws-dotnet-math1", Language::Dotnet)]
#[case::java_math1("et-ws-java-math1", Language::Java)]
#[case::js_math1("et-ws-js-math1", Language::Js)]
#[case::kotlin_math1("et-ws-kotlin-math1", Language::Kotlin)]
#[case::pymath1("et-ws-pymath1", Language::Python)]
#[case::zig_math1("et-ws-zig-math1", Language::Zig)]
#[tokio::test(flavor = "current_thread")]
async fn math1_module_stores_verified_model(#[case] module: &str, #[case] language: Language) {
    if !mise_env_includes(language) {
        println!(
            "skipping {module}: requires the `{}` mise env, not loaded",
            language.as_str()
        );
        return;
    }
    if module == "et-ws-dotnet-math1" && !dotnet_math1_pkg_built() {
        println!("skipping {module}: pkg/ not built (build-ws-dotnet-math1-module has not run on this host)");
        return;
    }
    if module == "et-ws-kotlin-math1" && !kotlin_math1_pkg_built() {
        println!("skipping {module}: pkg/ not built (build-ws-kotlin-math1-module has not run on this host)");
        return;
    }
    let server = et_ws_test_server::start();
    let bin = env!("CARGO_BIN_EXE_et-ws-web-runner");
    let mut runner = std::process::Command::new(bin)
        .env("RUNNER_MODULE", module)
        .env("WS_SERVER_URL", &server.ws_url)
        .env("RUNNER_TIMEOUT", "90s")
        .spawn()
        .unwrap();
    let outcome = et_ws_test_server::math1::drive_math1_exchange(
        &server.ws_url,
        server.storage_dir.path(),
        std::time::Duration::from_secs(90),
    )
    .await;
    // Reap the runner regardless of the exchange outcome; it exits on its own once the module
    // completes (RUNNER_TIMEOUT bounds a hung module).
    let status = wait_for_runner_exit(&mut runner);
    let (weight, bias) = outcome.unwrap_or_else(|err| panic!("{module}: {err}"));
    et_ws_test_server::math1::verify_math1_model(weight, bias).unwrap_or_else(|err| panic!("{module}: {err}"));
    assert!(status.success(), "{module} runner exited {status:?}");
    #[cfg(feature = "coverage")]
    collect_module_coverage(&server);
}

/// The manual-trigger pair: math1-sender plays the fake agent while the Rust math1 twin computes.
///
/// Both modules spawn against one server. The sender uploads the embedded canonical input,
/// broadcasts the pointer (for a minute -- its manual-use window, which this test's wall clock
/// includes), and math1 stores its model, which is then verified against the expected weights.
/// This is the end-to-end proof of the browser-only trigger path, with no harness fake agent.
#[test]
fn math1_sender_triggers_math1() {
    if !mise_env_includes(Language::Rust) {
        println!("skipping et-ws-math1-sender: requires the `rust` mise env, not loaded");
        return;
    }
    let server = et_ws_test_server::start();
    let bin = env!("CARGO_BIN_EXE_et-ws-web-runner");
    let spawn = |module: &str| {
        std::process::Command::new(bin)
            .env("RUNNER_MODULE", module)
            .env("WS_SERVER_URL", &server.ws_url)
            .env("RUNNER_TIMEOUT", "110s")
            .spawn()
            .unwrap()
    };
    let mut sender = spawn("et-ws-math1-sender");
    let mut math1 = spawn("et-ws-math1");
    let math1_status = wait_for_runner_exit(&mut math1);
    let sender_status = wait_for_runner_exit(&mut sender);
    assert!(math1_status.success(), "math1 runner exited {math1_status:?}");
    assert!(sender_status.success(), "math1-sender runner exited {sender_status:?}");

    // The twin stored its model in its own bucket; find it and verify the weights.
    let mut verified = false;
    for bucket in fs_err::read_dir(server.storage_dir.path()).unwrap().flatten() {
        let output_path = bucket.path().join("math1-output.json");
        if let Ok(bytes) = fs_err::read(&output_path) {
            let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let weight = value.get("weight").and_then(serde_json::Value::as_f64).unwrap();
            let bias = value.get("bias").and_then(serde_json::Value::as_f64).unwrap();
            et_ws_test_server::math1::verify_math1_model(weight, bias).unwrap();
            verified = true;
        }
    }
    assert!(verified, "no math1-output.json found in any storage bucket");
    #[cfg(feature = "coverage")]
    collect_module_coverage(&server);
}

/// Poll the spawned runner until it exits, killing it if it overstays the runner-timeout bound.
///
/// Blocking here is fine: this runs after the exchange future has already resolved, so nothing
/// else is pending on the current-thread runtime.
#[expect(
    clippy::arithmetic_side_effects,
    reason = "the deadline addition cannot overflow within a test's lifetime"
)]
fn wait_for_runner_exit(runner: &mut std::process::Child) -> std::process::ExitStatus {
    let deadline = std::time::Instant::now() + std::time::Duration::from_mins(2);
    loop {
        if let Some(status) = runner.try_wait().unwrap() {
            return status;
        }
        if std::time::Instant::now() >= deadline {
            runner.kill().unwrap();
            return runner.wait().unwrap();
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Probe for dotnet-data1's built `pkg/` wasm artifacts, logging a skip instead of failing when absent.
///
/// dotnet-data1's `pkg/` wasm artifacts only exist after `build-ws-dotnet-data1-module` has run on this
/// host, so probe one and log a skip instead of failing on a checkout where the module wasn't built. The
/// probe file is `dotnet.js` -- the one stably-named artifact `dotnet publish` emits (the rest carry
/// content-hash fingerprints) -- and NOT `package.json`, which is committed and therefore always present.
#[expect(
    clippy::single_call_fn,
    reason = "distinct probe step; kept named for the skip-trace log line"
)]
fn dotnet_data1_pkg_built() -> bool {
    edge_toolkit::config::get_project_root()
        .join("services/ws-modules/dotnet-data1/pkg/dotnet.js")
        .exists()
}

/// Probe for dotnet-math1's built `pkg/` wasm artifacts, logging a skip instead of failing when absent.
///
/// Same shape as [`dotnet_data1_pkg_built`]: probe the stably-named `dotnet.js` that
/// `build-ws-dotnet-math1-module` copies into `pkg/`, and skip rather than 404 on the fetch.
#[expect(
    clippy::single_call_fn,
    reason = "distinct probe step; kept named for the skip-trace log line"
)]
fn dotnet_math1_pkg_built() -> bool {
    edge_toolkit::config::get_project_root()
        .join("services/ws-modules/dotnet-math1/pkg/dotnet.js")
        .exists()
}

/// Probe for js-data1's esbuild bundle, logging a skip instead of failing when absent.
///
/// The AWS SDK v3 bundle `pkg/et_ws_js_data1.js` is generated by `build-ws-js-data1-module` and gitignored, so
/// on a checkout where that task has not run it is absent. Probe it and skip rather than 404 on the fetch. The
/// committed `pkg/package.json` is always present, so it is not a useful probe.
#[expect(
    clippy::single_call_fn,
    reason = "distinct probe step; kept named for the skip-trace log line"
)]
fn js_data1_pkg_built() -> bool {
    edge_toolkit::config::get_project_root()
        .join("services/ws-modules/js-data1/pkg/et_ws_js_data1.js")
        .exists()
}

/// Probe for kotlin-data1's Kotlin/Wasm build output, logging a skip instead of failing when absent.
///
/// The `WasmGC` module and its loader glue (`pkg/et_ws_kotlin_data1_compiled.*`) are generated by
/// `build-ws-kotlin-data1-module` and gitignored, so on a checkout where that task has not run they are
/// absent. Probe the loader and skip rather than 404 on the fetch. The committed `pkg/package.json` is always
/// present, so it is not a useful probe.
#[expect(
    clippy::single_call_fn,
    reason = "distinct probe step; kept named for the skip-trace log line"
)]
fn kotlin_data1_pkg_built() -> bool {
    edge_toolkit::config::get_project_root()
        .join("services/ws-modules/kotlin-data1/pkg/et_ws_kotlin_data1_compiled.mjs")
        .exists()
}

/// Probe for kotlin-math1's Kotlin/Wasm build output, logging a skip instead of failing when absent.
///
/// Same shape as [`kotlin_data1_pkg_built`]: the `WasmGC` module and loader glue are generated by
/// `build-ws-kotlin-math1-module` and gitignored, so probe the loader and skip rather than 404 on the fetch.
#[expect(
    clippy::single_call_fn,
    reason = "distinct probe step; kept named for the skip-trace log line"
)]
fn kotlin_math1_pkg_built() -> bool {
    edge_toolkit::config::get_project_root()
        .join("services/ws-modules/kotlin-math1/pkg/et_ws_kotlin_math1_compiled.mjs")
        .exists()
}

/// Spawn two runners against one ws-server and assert both finish ok.
/// Used by communication modules that need to discover at least one peer
/// via `et-list-agents` before they can complete (comm1, dart-comm1).
#[rstest]
#[case::comm1("et-ws-comm1", Language::Rust)]
#[case::dart_comm1("et-ws-dart-comm1", Language::Dart)]
fn multi_agent_module(#[case] module: &str, #[case] language: Language) {
    if !mise_env_includes(language) {
        println!(
            "skipping {module}: requires the `{}` mise env, not loaded",
            language.as_str()
        );
        return;
    }
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

/// Load + run each hardware/sensor module and assert it fails without the device API it needs.
///
/// The modules listed under "Hardware / browser-only APIs" above touch a browser sensor/media API the runner's
/// shims deliberately do NOT provide (only `navigator.userAgent` is stubbed) -- camera `getUserMedia`,
/// `DeviceMotionEvent`, geolocation, `navigator.bluetooth`, `NDEFReader`, `webkitSpeechRecognition`. So they
/// cannot complete under Deno, which is why they are excluded from `module_runs_successfully`. Running them
/// anyway still exercises the fetch + module-graph load + entry-evaluation paths -- in the runner AND in each
/// module's wasm-bindgen glue up to the point it reaches for the missing API -- which is otherwise uncovered
/// (har1 and face-detection especially). We assert a non-zero exit: the module either throws when the absent API
/// is touched or is killed by `RUNNER_TIMEOUT` while awaiting a device callback that never fires. A module that
/// unexpectedly EXITS 0 here is a real finding -- the runner can now run it, so move it to
/// `module_runs_successfully`.
#[rstest]
#[case::audio1("et-ws-audio1", Language::Rust)]
#[case::bluetooth("et-ws-bluetooth", Language::Rust)]
#[case::face_detection("et-ws-face-detection", Language::Rust)]
#[case::geolocation("et-ws-geolocation", Language::Rust)]
#[case::har1("et-ws-har1", Language::Rust)]
#[case::llm1("et-ws-llm1", Language::Rust)]
#[case::nfc("et-ws-nfc", Language::Rust)]
#[case::sensor1("et-ws-sensor1", Language::Rust)]
#[case::speech_recognition("et-ws-speech-recognition", Language::Rust)]
#[case::video1("et-ws-video1", Language::Rust)]
#[case::pyface1("et-ws-pyface1", Language::Python)]
#[case::pydemo1("et-ws-pydemo1", Language::Python)]
#[case::pyeye1("et-ws-pyeye1", Language::Python)]
#[case::pyspeech1("et-ws-pyspeech1", Language::Python)]
fn hardware_module_load_fails(#[case] module: &str, #[case] language: Language) {
    if !mise_env_includes(language) {
        println!(
            "skipping {module}: requires the `{}` mise env, not loaded",
            language.as_str()
        );
        return;
    }
    let server = et_ws_test_server::start();
    // A missing sensor/media API throws promptly once the module runs, so the module usually exits well under
    // this bound; it only bites if a module instead awaits a device callback that never fires, in which case
    // RUNNER_TIMEOUT kills it -- still the non-zero exit we assert.
    let output = run_runner(module, &server.ws_url, 30);
    #[cfg(feature = "coverage")]
    collect_module_coverage(&server);
    assert!(
        !output.status.success(),
        concat!(
            "{} exited 0, but it was expected to fail without its browser sensor/media API. ",
            "If the runner can now run it, move it to `module_runs_successfully`.\n",
            "--- stdout ---\n{}\n--- stderr ---\n{}",
        ),
        module,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// Load + run each R module and assert it fails under Deno's classic-worker limitation.
///
/// rdata1 and rcomm1 run R through webR, whose `WebR.init()` spawns the R interpreter on a *classic* Web Worker.
/// Deno implements only module workers, so the spawn throws `NotSupportedError: Classic workers are not
/// supported` and the module aborts in its `init()` default export -- before any WebSocket, peer-discovery, or
/// storage logic runs (so one runner suffices even for the comm-style rcomm1). Running them here still exercises
/// the fetch + module-graph load + entry-evaluation paths up to the failing `new Worker(...)`. We assert a
/// non-zero exit; a module that unexpectedly EXITS 0 is a real finding -- the runner gained classic-worker
/// support, so move it to `module_runs_successfully`. Both modules work in a real browser (classic workers +
/// the `SharedArrayBuffer` channel the ws-server's COOP/COEP headers enable).
#[rstest]
#[case::rdata1("et-ws-rdata1", Language::R)]
#[case::rcomm1("et-ws-rcomm1", Language::R)]
#[case::rmath1("et-ws-rmath1", Language::R)]
fn r_module_load_fails(#[case] module: &str, #[case] language: Language) {
    if !mise_env_includes(language) {
        println!(
            "skipping {module}: requires the `{}` mise env, not loaded",
            language.as_str()
        );
        return;
    }
    let server = et_ws_test_server::start();
    // webR's classic-worker spawn throws promptly in init(), so the module exits well under this bound; the
    // budget only bites if webR instead hangs before that point, in which case RUNNER_TIMEOUT kills it -- still
    // the non-zero exit we assert.
    let output = run_runner(module, &server.ws_url, 30);
    #[cfg(feature = "coverage")]
    collect_module_coverage(&server);
    assert!(
        !output.status.success(),
        concat!(
            "{} exited 0, but it was expected to fail: webR spawns a classic Worker, which Deno does not ",
            "support. If the runner can now run it, move it to `module_runs_successfully`.\n",
            "--- stdout ---\n{}\n--- stderr ---\n{}",
        ),
        module,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// Spawn one `et-ws-web-runner` against `ws_url` and return its captured output.
///
/// `timeout_secs` is passed as `RUNNER_TIMEOUT` (humantime, e.g. `120s`); the
/// multi-agent harness bumps it because two cold V8 starts contending for the
/// same box widen the discovery window past the single-agent budget.
fn run_runner(module: &str, ws_url: &str, timeout_secs: u32) -> std::process::Output {
    let bin = env!("CARGO_BIN_EXE_et-ws-web-runner");
    // ET_TEST_COVERAGE, when set by the coverage workflow, is inherited by the child (Command keeps the parent
    // env), so the Pyodide shims collect coverage into ws-server storage -- no explicit forwarding needed.
    std::process::Command::new(bin)
        .env("RUNNER_MODULE", module)
        .env("WS_SERVER_URL", ws_url)
        .env("RUNNER_TIMEOUT", format!("{timeout_secs}s"))
        .output()
        .unwrap()
}

/// Collect the coverage a module PUT into the test server's storage.
///
/// Each module writes its coverage to its own agent bucket (`<agent_id>/`), because the storage `put_file` only
/// accepts a registered agent as the bucket. So this scans every agent bucket under the storage dir and routes
/// files by extension into the two later coverage tasks:
/// - `.coverage` (Pyodide coverage.py data) -> `target/pycov/` renamed to coverage.py's parallel-data
///   convention (`.coverage.<pkg>`) for the `pytest-cov` combine.
/// - `.profraw` (Rust browser-wasm minicov) -> `target/wasi-cov/` (where the `wasm-cov` task turns each into
///   lcov via the same llc/llvm-cov pipeline).
///
/// Compiled in only under the `coverage` feature (like the runner's capture code); the call sites are gated to
/// match. Without a coverage build this whole helper is absent, and a plain test run never captures.
#[cfg(feature = "coverage")]
fn collect_module_coverage(server: &et_ws_test_server::TestServer) {
    let root = edge_toolkit::config::get_project_root();
    let Ok(buckets) = fs::read_dir(server.storage_dir.path()) else {
        return;
    };
    for bucket in buckets.flatten() {
        let Ok(entries) = fs::read_dir(bucket.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match path.extension().and_then(|ext| ext.to_str()) {
                Some("coverage") => {
                    let dest_dir = root.join("target/pycov");
                    fs::create_dir_all(&dest_dir).unwrap();
                    let stem = path.file_stem().unwrap().to_string_lossy();
                    let _copied = fs::copy(&path, dest_dir.join(format!(".coverage.{stem}"))).unwrap();
                }
                Some("profraw") => {
                    let dest_dir = root.join("target/wasi-cov");
                    fs::create_dir_all(&dest_dir).unwrap();
                    let name = path.file_name().unwrap();
                    let _copied = fs::copy(&path, dest_dir.join(name)).unwrap();
                }
                _ => {}
            }
        }
    }
}

/// Run `module` and panic with the captured stdout/stderr on non-zero exit.
fn run_runner_with_timeout(module: &str, ws_url: &str, timeout_secs: u32) {
    let output = run_runner(module, ws_url, timeout_secs);
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
