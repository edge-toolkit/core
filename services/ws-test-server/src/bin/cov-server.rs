//! Long-lived in-process ws-server for the `wasm-agent-cov` coverage task.
//!
//! Starts the same hub the integration tests use on a fixed port (the one the browser wasm-agent tests connect
//! to), writes a readiness marker to the file named by the first CLI argument once the server is accepting, then
//! parks so the headless-browser tests can drive a real backend. The parent mise task waits for the marker and
//! kills this process afterwards.

use std::error::Error;
use std::path::PathBuf;

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

    // Park indefinitely, keeping `server` (its worker thread + temp storage dir) alive until the task kills us.
    #[expect(
        clippy::infinite_loop,
        reason = "a launcher that intentionally stays up until the task kills it"
    )]
    loop {
        std::thread::park();
    }
}
