#![cfg(test)]
//! End-to-end check that the `math1` scenario's generated runner tasks actually run their modules.
//!
//! This is deployment verification, not module verification. `services/ws-web-runner/tests/modules.rs` already
//! proves the math1 pair computes the right model when a test spawns the runners itself; what that leaves
//! unproven is whether the tasks `et-cli` *generates* spawn them correctly -- the task names, the `RUNNER_MODULE`
//! values and the readiness wait are all this generator's output, and running the modules directly exercises
//! none of them. So the runners are started with `mise run` against the committed
//! `verification/local/output/math1/mise.toml` rather than by reimplementing what those tasks do. A generator
//! change that renames a task, emits the wrong module name, or drops the readiness wait fails here.
//!
//! The hub is the in-process `et-ws-test-server` rather than the generated `ws-server` task, and that is
//! deliberate. Going through `mise run` for the hub means `mise` spawns `cargo run` spawns the server, and
//! killing the top of that chain orphans the server, which then holds the port and makes the NEXT run of this
//! test find a stale listener, satisfy its readiness check against the wrong process and time out with nothing
//! stored -- observed exactly that way before this rewrite. An in-process hub has no tree to reap, brings its own
//! temporary storage root so no previous run's output can be mistaken for this one's, and is the same server the
//! sibling runner tests use.

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
fn spawn_runner(task: &str) -> Runner {
    let scenario_dir = edge_toolkit::config::get_project_root().join("verification/local/output/math1");
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

/// Build the runner binary before any of the timed work starts.
///
/// The generated task runs `cargo run --quiet -p et-ws-web-runner`, and that build is not free here: nextest
/// compiles the workspace under the `test` profile while `cargo run` uses `dev`, so the first task invocation
/// links the web runner -- V8 and all -- from scratch. Left inside the exchange window it consumed 117s of a 180s
/// budget on CI, and the model was stored just after the poll loop gave up. The failure then read as
/// "no math1-output.json appeared", which looks like a broken deployment rather than a test timing its own
/// compiler. Paying for the build up front keeps the deadline measuring the exchange and nothing else.
#[expect(
    clippy::single_call_fn,
    reason = "distinct setup step; separate so the test body reads as hub, runners, exchange"
)]
fn prebuild_runner() {
    let _status = Command::new("cargo")
        .args(["build", "--quiet", "-p", "et-ws-web-runner"])
        .current_dir(edge_toolkit::config::get_project_root())
        .status_checked()
        .unwrap();
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
    // Build the runner before anything is timed.
    prebuild_runner();

    // The generated tasks name the hub's standard port, so the hub has to be on that port rather than a
    // reserved one. `start_on` fails loudly if it is already taken, which is the right outcome: a leftover
    // server would otherwise answer the runners' readiness check and quietly serve a different storage root.
    let server = et_ws_test_server::start_on(Services::InsecureWebSocketServer.port());
    let storage_dir = server.storage_dir.path();

    // Both runners come up together, exactly as `generated-scenario` starts them.
    let mut twin = spawn_runner("math1-twin");
    let mut trigger = spawn_runner("math1-trigger");

    let deadline = Instant::now() + EXCHANGE_TIMEOUT;
    let mut model = None;
    while Instant::now() < deadline {
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
                "no math1-output.json appeared in any storage bucket under {}\n",
                "--- math1-twin stdout ---\n{}\n--- math1-twin stderr ---\n{}\n",
                "--- math1-trigger stdout ---\n{}\n--- math1-trigger stderr ---\n{}"
            ),
            storage_dir.display(),
            captured(&twin.stdout),
            captured(&twin.stderr),
            captured(&trigger.stdout),
            captured(&trigger.stderr)
        );
    };
    et_ws_test_server::math1::verify_math1_model(weight, bias).unwrap();
    assert!(twin_exited, "math1-twin did not exit within its RUNNER_TIMEOUT");
    assert!(trigger_exited, "math1-trigger did not exit within its RUNNER_TIMEOUT");
}
