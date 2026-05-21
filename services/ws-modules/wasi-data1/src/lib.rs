//! Rust WASI Preview 2 port of the data1 workflow module.
//!
//! Browser data1 (`services/ws-modules/data1`) round-trips a file through
//! the ws-server's storage service by:
//!   1. Asking the server (via `StoreFile`) for a PUT URL,
//!   2. HTTP-PUTting bytes,
//!   3. Asking (via `FetchFile`) for a GET URL,
//!   4. HTTP-GETting and verifying.
//!
//! The WASI runner doesn't expose generic HTTP; the equivalent here uses
//! `wasi:keyvalue/store` directly. The bucket identifier is the agent's
//! own `agent_id`, which maps host-side to `/storage/{agent_id}/{key}` —
//! same backend store, same auth boundary (writes only succeed inside
//! one's own bucket), one fewer protocol hop.
//!
//! Crate-level cfg gate: the wit-bindgen-generated extern declarations
//! reference WASI imports that only resolve on `wasm32-wasip2`. Gating the
//! whole module on `target_os = "wasi"` lets the crate sit in the parent
//! workspace — `cargo check --workspace` from the repo root produces an
//! empty cdylib for the host target without linker errors.

#![cfg(target_os = "wasi")]

wit_bindgen::generate!({
    path: "../../ws-wasi-runner/wit",
    world: "module",
    generate_all,
});

use et::ws_wasi::ws::WsError;
use exports::et::ws_wasi::entry::{Guest, RunError};
use wasi::keyvalue::store;
use wasi::logging::logging::{self, Level};

const LOG_CONTEXT: &str = env!("CARGO_PKG_NAME");
const FILENAME: &str = "test_data.txt";

fn info(message: &str) {
    logging::log(Level::Info, LOG_CONTEXT, message);
}

// wit-bindgen-generated error types don't implement `Error`, so
// thiserror's `#[from]` can't drive these conversions. Handwritten
// `From` impls let `?` flatten host-import errors straight into the
// matching `RunError` variant.
impl From<WsError> for RunError {
    fn from(source: WsError) -> Self {
        RunError::Ws(format!("{source:?}"))
    }
}

impl From<store::Error> for RunError {
    fn from(source: store::Error) -> Self {
        RunError::Store(format!("{source:?}"))
    }
}

struct Component;

impl Guest for Component {
    fn run() -> Result<(), RunError> {
        info("entered run()");

        et::ws_wasi::ws::connect()?;
        let agent_id = wait_for_agent_id().ok_or_else(|| RunError::Precondition("did not receive agent_id".into()))?;
        info(&format!("websocket connected with agent_id={agent_id}"));

        let bucket = store::open(&agent_id)?;

        let test_content = format!("Hello from wasi-data1, agent={agent_id}!").into_bytes();
        info(&format!("storing {} bytes to key {FILENAME}", test_content.len()));
        bucket.set(FILENAME, &test_content)?;

        info(&format!("fetching key {FILENAME}"));
        let fetched = bucket
            .get(FILENAME)?
            .ok_or_else(|| RunError::Precondition(format!("bucket.get({FILENAME}) returned none after set")))?;

        if fetched != test_content {
            return Err(RunError::Precondition(format!(
                "data mismatch: sent {} bytes, got {} bytes",
                test_content.len(),
                fetched.len()
            )));
        }
        info("VERIFICATION SUCCESS — keyvalue roundtrip matches");

        et::ws_wasi::ws::disconnect();
        info("workflow complete");
        Ok(())
    }
}

/// `ws.connect` waits briefly for the `ConnectAck` server message, but the
/// host returns once that wait expires regardless. Poll `agent_id` to be
/// safe under load.
fn wait_for_agent_id() -> Option<String> {
    for _ in 0..100 {
        let id = et::ws_wasi::ws::agent_id();
        if !id.is_empty() {
            return Some(id);
        }
        sleep_ms(50);
    }
    None
}

fn sleep_ms(ms: u64) {
    let pollable = wasi::clocks::monotonic_clock::subscribe_duration(ms * 1_000_000);
    wasi::io::poll::poll(&[&pollable]);
}

export!(Component);
