//! End-to-end check that each math1 scenario's generated runner tasks actually run their modules.
//!
//! This is deployment verification, not module verification. Each runner's own `tests/modules.rs` already
//! proves its math1 twin computes the right model when a test spawns it directly; what that leaves unproven is
//! whether the tasks `et-cli` *generates* spawn them correctly -- the task names, the `RUNNER_MODULE` values and
//! the readiness wait are all this generator's output, and running the modules directly exercises none of them.
//! So the runners are started with `mise run` against the committed `verification/local/output/<scenario>/
//! mise.toml` rather than by reimplementing what those tasks do. A generator change that renames a task, emits
//! the wrong module name, or drops the readiness wait fails here.
//!
//! One test per runner kind, because the kinds are what the generator varies: a wasm twin in the web runner, a
//! WASI component in the wasi runner, and native `CPython` in the pyo3 runner. All three compute the same model
//! from the same input -- the `FedAvg` kernel is float arithmetic only -- so one expectation covers them, and a
//! scenario whose twin never stores is a deployment fault rather than a disagreement about the answer. Three
//! separate tests rather than one parameterised over the kinds, because they no longer share a platform gate:
//! each is skipped, or not, on its own evidence.
//!
//! The trigger differs by scenario and that is the point of having two senders: `math1` is driven by the
//! browser-targeted `math1-sender`, while the other two use `wasi-math1-sender`, so a deployment whose twin
//! never touches a browser does not have to start a web runner merely to be triggered.
//!
//! The hub is the in-process `et-ws-test-server` rather than the generated `ws-server` task, and that is
//! deliberate. Going through `mise run` for the hub means `mise` spawns `cargo run` spawns the server, and
//! killing the top of that chain orphans the server, which then holds the port and makes the NEXT run of this
//! test find a stale listener, satisfy its readiness check against the wrong process and time out with nothing
//! stored -- observed exactly that way before this rewrite. An in-process hub has no tree to reap, brings its own
//! temporary storage root so no previous run's output can be mistaken for this one's, and is the same server the
//! sibling runner tests use.
#![cfg(test)]

use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use command_error::CommandExt as _;
use edge_toolkit::ports::Services;
use et_test_helpers::{ChildGuard, drain_stderr, drain_stdout};
use fs_err as fs;

/// Wall-clock ceiling for the whole exchange once the hub is up.
///
/// The sender broadcasts its pointer once a second across its manual-use window, and the twin stores its model on
/// the first one it sees, so this only has to outlast the runners' own startup.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(180);

/// `RUNNER_TIMEOUT` handed to both runners so they exit on their own rather than running until killed.
///
/// The generated tasks deliberately set no timeout -- a deployed cluster runs until it is stopped -- so this
/// bound belongs to the test. It matches the 110s that `services/ws-web-runner/tests/modules.rs` gives the same
/// pair, and that figure is not padding: a shorter bound was tried and killed the sender before the exchange
/// finished on CI, where module fetch and Deno startup are far slower than on a workstation. The test waits this
/// out, so it also sets how long the test takes.
const RUNNER_TIMEOUT: &str = "110s";

/// Ceiling for waiting out a runner after the model has been stored.
///
/// Must stay comfortably longer than [`RUNNER_TIMEOUT`], because that is what actually ends the runner and this
/// only catches one that ignores it. Set below that bound, the wait would kill a runner that was about to exit
/// on its own and then report it as a timeout.
const RUNNER_EXIT_TIMEOUT: Duration = Duration::from_secs(150);

/// A spawned runner task, with both its output streams captured for the failure message.
struct Runner {
    guard: ChildGuard,
    stdout: Arc<Mutex<String>>,
    stderr: Arc<Mutex<String>>,
}

/// Spawn one of the generated tasks, with only the runner bound layered on top.
///
/// Nothing the task declares is overridden: `RUNNER_MODULE` and `WS_SERVER_URL` have to keep coming from the
/// generated file, because those values are exactly what is under test. `mise` is spawned by name because the
/// README mandates it, so it is on `PATH` in every sanctioned environment.
///
/// BOTH streams are piped and drained rather than discarded, and that has already been got wrong twice.
/// First everything went to `Stdio::null()`, so a CI failure said only "no math1-output.json appeared" with no
/// hint why. Then stderr alone was captured -- which caught a runner's exit error, but the runner writes its
/// `tracing` output to stdout, so the next failure came back with an empty capture and nothing to go on. The
/// drained buffers fill once the child reaches EOF, which is after the wait below, so the failure can quote both.
fn spawn_runner(scenario: &str, task: &str) -> Runner {
    let scenario_dir = edge_toolkit::config::get_project_root().join(format!("verification/local/output/{scenario}"));
    let mut child = Command::new("mise")
        .arg("run")
        .arg(task)
        .current_dir(scenario_dir)
        .env("RUNNER_TIMEOUT", RUNNER_TIMEOUT)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn_checked()
        .unwrap()
        .into_child();
    let stdout = drain_stdout(&mut child);
    let stderr = drain_stderr(&mut child);
    Runner {
        guard: ChildGuard::new(child),
        stdout,
        stderr,
    }
}

/// Build the runner binaries before any of the timed work starts.
///
/// The generated tasks run `cargo run --quiet -p <crate>`, and that build is not free here: nextest compiles
/// the workspace under the `test` profile while `cargo run` uses `dev`, so the first task invocation links the
/// runner -- V8 and all, for the web one -- from scratch. Left inside the exchange window it consumed 117s of a
/// 180s budget on CI, and the model was stored just after the poll loop gave up. The failure then read as
/// "no math1-output.json appeared", which looks like a broken deployment rather than a test timing its own
/// compiler. Paying for the build up front keeps the deadline measuring the exchange and nothing else.
///
/// Both halves are named, because a scenario's trigger and twin can run on different runners.
#[expect(
    clippy::single_call_fn,
    reason = "distinct setup step; separate so the test body reads as hub, runners, exchange"
)]
fn prebuild_runners(trigger_crate: &str, twin_crate: &str) {
    for package in [trigger_crate, twin_crate] {
        let _status = Command::new("cargo")
            .args(["build", "--quiet", "-p", package])
            .current_dir(edge_toolkit::config::get_project_root())
            .status_checked()
            .unwrap();
    }
}

/// Read a drained stderr buffer, tolerating a panic in the draining thread having poisoned it.
fn captured(buffer: &Arc<Mutex<String>>) -> String {
    buffer
        .lock()
        .map_or_else(|poisoned| poisoned.into_inner().clone(), |guard| guard.clone())
}

/// Return the first stored `math1-output.json` as `(weight, bias)`, or `None` until the twin has written one.
#[expect(
    clippy::single_call_fn,
    reason = "distinct step of the exchange, kept out of the poll loop for readability"
)]
fn stored_model(storage_dir: &std::path::Path) -> Option<(f64, f64)> {
    for bucket in fs::read_dir(storage_dir).ok()?.flatten() {
        let Ok(bytes) = fs::read(bucket.path().join("math1-output.json")) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        let weight = value.get("weight").and_then(serde_json::Value::as_f64);
        let bias = value.get("bias").and_then(serde_json::Value::as_f64);
        if let (Some(weight), Some(bias)) = (weight, bias) {
            return Some((weight, bias));
        }
    }
    None
}

// Not run on Windows, with the operator's sign-off, because that lane does not build the crate this exercises.
// `config/config.windows.toml` sets `cargo_ws_excludes = "--exclude et-ws-web-runner"`, so the Windows test run
// is literally `cargo nextest run --workspace --exclude et-ws-web-runner ...` -- while the generated task this
// test drives runs `cargo run --quiet -p et-ws-web-runner`. Asking Windows to build the one crate CI has decided
// to skip there is not a gap this test can close; that is the gnullvm rusty_v8 work, tracked separately.
//
// Observed on commit 809600492822c600f14c19a35b1ff50bef687c23 at
// https://github.com/edge-toolkit/core/actions/runs/34108671291/job/101699520407 as
//   FAIL + LEAK [ 363.640s] (177/177) et-cli::scenario_runners math1_scenario_generated_runner_tasks_compute_the_model
//   no math1-output.json appeared in any storage bucket under C:\Users\RUNNER~1\AppData\Local\Temp\.tmpKcKuqX
// where every other test in that run passed (176 passed, 1 failed, 6 skipped).
//
// `ignore` rather than `cfg`, so the test stays listed and compiled on Windows: the body is platform-agnostic,
// and a silently absent test is what this comment exists to avoid. Drop the attribute once the Windows lane
// stops excluding et-ws-web-runner.
#[test]
#[cfg_attr(
    windows,
    ignore = "Windows CI excludes et-ws-web-runner, the crate the generated runner task builds"
)]
fn math1_scenario_generated_runner_tasks_compute_the_model() {
    run_scenario("math1", "math1-twin", "et-ws-web-runner", "et-ws-web-runner");
}

// Neither of the two tests below runs on the mingw lane, with the operator's sign-off, because
// `et-ws-wasi-runner` does not work on `x86_64-pc-windows-gnu`, and it is the trigger for both of them. The
// evidence is shared rather than repeated per test: the signature is the same one, and writing it out twice
// would duplicate ~25 lines inside a single file. A runner process gets as far as instantiating the component
// and logging `entered run()`, then aborts while connecting:
//
//   thread 'main' panicked at libs\ws-runner-common\src\lib.rs:206:5:
//   there is no reactor running, must be called from the context of a Tokio 1.x runtime
//   thread 'main' panicked at tokio-1.53.1\src\runtime\context\runtime.rs:85:13:
//   assertion failed: c.runtime.get().is_entered()
//   panic in a destructor during cleanup / thread caused non-unwinding panic. aborting.
//   cargo.exe: The system detected an overrun of a stack-based buffer in this application. Error 0xc0000409
//
// That line is `tokio::time::timeout` inside `connect_and_register`, reached from the `ws::connect` host
// import -- an async host call awaited on a wasmtime fiber. The reading that fits is tokio's thread-local
// runtime context not surviving the fiber stack switch under that target's TLS model.
//
// It is the target env, not Windows. On commit 5998313315c491a6abffb7c1507e4adc4a4f3559 `wasi-math1` passed on
// `gnullvm` (which `config.windows.toml` actually builds) in 426s and on `msvc` in 443s, while `gnu` failed
// twice with the identical signature -- 644s at
// https://github.com/edge-toolkit/core/actions/runs/34187567425/job/101938939834 and 591s on the re-run at
// https://github.com/edge-toolkit/core/actions/runs/34187567425/job/101958844541 -- so it is reproducible
// rather than a flake. `pyo3-math1` was left ungated at that point because fail-fast had cancelled it before
// it ran on `gnu`; it then reproduced the same abort there at 579s on commit
// 29dfe80a62ba7a27d8119c5b6332c3dbe2df815e,
// https://github.com/edge-toolkit/core/actions/runs/34211905976/job/102014621776, having passed on `gnullvm` in
// 472s and `msvc` in 458s. Its captured output puts the fault squarely on the trigger: the pyo3 twin registers
// as an agent and idles to its own timeout, while the wasi trigger aborts as above.
//
// The defect predates these tests. Only `services/ws-wasi-runner/tests/modules.rs` and `otel_propagation.rs`
// instantiate the runner at all -- the other two files in that directory drive `openobserve` and `vector` --
// and both are `#[cfg_attr(windows, ignore)]` for an unrelated `pkg/package.json` 404, so the runner had never
// run on any Windows lane and nothing had ever exercised this path there.
//
// Gated on the target rather than on `windows`, so the two targets that work keep running it -- and `gnu` alone
// is not that gate. `x86_64-pc-windows-gnullvm`, the default this repo builds, reports `target_env = "gnu"` too
// and is separated from mingw only by its ABI:
//
//   x86_64-pc-windows-gnullvm    target_abi="llvm" target_env="gnu"
//   x86_64-pc-windows-gnu        target_abi=""     target_env="gnu"
//
// so `not(target_abi = "llvm")` is what narrows it to mingw. Without that clause the default Windows lane
// skipped these too, silently: run 34265416244 ran 190 tests in 46.9s with no `scenario_runners` at all, where
// the run before it had passed `pyo3_math1_scenario` there in 472.5s.
//
// `ignore` rather than `cfg` keeps the test listed and compiled everywhere. Drop the attribute once the runner
// works on that target -- the test body has no platform dependence of its own.
#[test]
#[cfg_attr(
    all(windows, target_env = "gnu", not(target_abi = "llvm")),
    ignore = "et-ws-wasi-runner aborts on x86_64-pc-windows-gnu; gnullvm and msvc pass"
)]
fn wasi_math1_scenario_generated_runner_tasks_compute_the_model() {
    run_scenario(
        "wasi-math1",
        "wasi-math1-twin",
        "et-ws-wasi-runner",
        "et-ws-wasi-runner",
    );
}

/// The cross-runtime scenario: a WASI trigger driving a native-`CPython` twin.
///
/// Gated for the trigger's sake only -- the pyo3 runner itself is fine on that target, and is left doing
/// nothing once the sender that would have broadcast to it has gone.
#[test]
#[cfg_attr(
    all(windows, target_env = "gnu", not(target_abi = "llvm")),
    ignore = "trigger et-ws-wasi-runner aborts on x86_64-pc-windows-gnu; gnullvm and msvc pass"
)]
fn pyo3_math1_scenario_generated_runner_tasks_compute_the_model() {
    run_scenario(
        "pyo3-math1",
        "pyo3-math1-twin",
        "et-ws-pyo3-runner",
        "et-ws-wasi-runner",
    );
}

/// Start one scenario's generated trigger and twin tasks against an in-process hub, and verify the model.
fn run_scenario(scenario: &str, twin_task: &str, twin_crate: &str, trigger_crate: &str) {
    // Build the runners before anything is timed.
    prebuild_runners(trigger_crate, twin_crate);

    // The generated tasks name the hub's standard port, so the hub has to be on that port rather than a
    // reserved one. `start_on` fails loudly if it is already taken, which is the right outcome: a leftover
    // server would otherwise answer the runners' readiness check and quietly serve a different storage root.
    //
    // Every test in this file therefore wants that one port, and none of them may overlap. That is enforced by
    // `config/nextest.toml`, which puts this binary in a `max-threads = 1` test group -- not by anything here.
    // An in-process lock would be the wrong instrument and would not work: nextest runs each test in its own
    // process, so a `Mutex` or `OnceLock` in this file guards nothing across the tests it appears to guard. The
    // repo runs tests only through nextest with that config (`mise run cargo-test`); a bare `cargo nextest run`
    // that omits `--config-file config/nextest.toml` silently drops the group and the tests then race for the
    // port, which looks like a deployment bug and is not one.
    let server = et_ws_test_server::start_on(Services::InsecureWebSocketServer.port());
    let storage_dir = server.storage_dir.path();

    // Both runners come up together, exactly as `generated-scenario` starts them.
    let mut twin = spawn_runner(scenario, twin_task);
    let mut trigger = spawn_runner(scenario, "math1-trigger");

    // Elapsed-versus-budget rather than a computed deadline: comparing two `Duration`s needs no arithmetic on
    // an `Instant`, which the workspace's restriction lints would otherwise object to.
    let started = Instant::now();
    let mut model = None;
    while started.elapsed() < EXCHANGE_TIMEOUT {
        if let Some(found) = stored_model(storage_dir) {
            model = Some(found);
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    }

    // Wait the runners out before asserting, so the next run of this test starts from a clean slate.
    // Killing them instead would leave the real runner processes behind -- see `ChildGuard::wait_for_exit`.
    let twin_exited = twin.guard.wait_for_exit(RUNNER_EXIT_TIMEOUT);
    let trigger_exited = trigger.guard.wait_for_exit(RUNNER_EXIT_TIMEOUT);

    let Some((weight, bias)) = model else {
        panic!(
            concat!(
                "{}: no math1-output.json appeared in any storage bucket under {}\n",
                "--- {} stdout ---\n{}\n--- {} stderr ---\n{}\n",
                "--- math1-trigger stdout ---\n{}\n--- math1-trigger stderr ---\n{}"
            ),
            scenario,
            storage_dir.display(),
            twin_task,
            captured(&twin.stdout),
            twin_task,
            captured(&twin.stderr),
            captured(&trigger.stdout),
            captured(&trigger.stderr)
        );
    };
    et_ws_test_server::math1::verify_math1_model(weight, bias).unwrap();
    assert!(twin_exited, "{twin_task} did not exit within its RUNNER_TIMEOUT");
    assert!(trigger_exited, "math1-trigger did not exit within its RUNNER_TIMEOUT");
}
