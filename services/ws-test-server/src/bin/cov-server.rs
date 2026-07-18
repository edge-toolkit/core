//! Long-lived in-process ws-server for the `wasm-agent-cov` coverage task.
//!
//! Starts the same hub the integration tests use on a fixed port (the one the browser wasm-agent tests connect
//! to), writes a readiness marker to the file named by the first CLI argument once the server is accepting, then
//! stays up until the task deletes that marker -- its stop signal. Returning from `main` (rather than being
//! killed) lets the coverage-instrumented build flush its counters on a clean exit; a SIGKILL would drop them.

use std::error::Error;
use std::path::PathBuf;
use std::time::Duration;

use fs_err as fs;

/// Fixed port the browser wasm-agent tests dial (`ws://127.0.0.1:8080/ws`), matching the ws-server port the
/// existing `web.rs` end-to-end test and `ws-e2e-chrome` use.
const COV_SERVER_PORT: u16 = 8080;

fn main() -> Result<(), Box<dyn Error>> {
    let ready_path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: cov-server <ready-file>")?;

    let server = et_ws_test_server::start_on(COV_SERVER_PORT);
    fs::write(&ready_path, server.ws_url.as_bytes())?;

    // Stay up until the task removes the readiness marker, then fall through to a clean exit so coverage flushes.
    while ready_path.exists() {
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}
