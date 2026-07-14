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
//! own `agent_id`, which maps host-side to `/storage/{agent_id}/{key}` --
//! same backend store, same auth boundary (writes only succeed inside
//! one's own bucket), one fewer protocol hop.
//!
//! Crate-level cfg gate: the wit-bindgen-generated extern declarations
//! reference WASI imports that only resolve on `wasm32-wasip2`. Gating the
//! whole module on `target_os = "wasi"` lets the crate sit in the parent
//! workspace -- `cargo check --workspace` from the repo root produces an
//! empty cdylib for the host target without linker errors.

#![cfg(target_os = "wasi")]
// wit_bindgen::generate! emits `unsafe fn` and `#[export_name]` items;
// `export!(Component)` does the same. Both trip workspace
// `unsafe_code = "deny"` lint; expect it at crate scope because outer
// `#[expect]` on the macro invocations themselves doesn't propagate to
// the items they expand into.
#![expect(unsafe_code)]

wit_bindgen::generate!({
    // ET_WIT_DIR is the absolute path to generated/specs/wit, emitted by build.rs.
    path: env!("ET_WIT_DIR"),
    world: "module",
    generate_all,
});

use et::ws_wasi::ws::WsError;
use exports::et::ws_wasi::entry::{EntryError, Guest};
use wasi::keyvalue::store;
use wasi::logging::logging::{self, Level};

// Coverage dump lives in its own module so Codacy can exclude just that file (its minicov call is unsafe).
#[cfg(feature = "coverage")]
mod coverage;

const LOG_CONTEXT: &str = env!("CARGO_PKG_NAME");
const FILENAME: &str = "test_data.txt";

fn info(message: &str) {
    logging::log(Level::Info, LOG_CONTEXT, message);
}

// Lets `?` lift a `ws-error` into `entry-error.ws(...)` so the body of `run`
// stays free of explicit `.map_err`s (which the workspace's no-map-err
// ast-grep rule bans outside listed error.rs files anyway).
impl From<WsError> for EntryError {
    fn from(err: WsError) -> Self {
        Self::Ws(err)
    }
}

// Same idea for `wasi:keyvalue/store.error` -- the upstream type is a
// value variant (no resources involved), so `entry-error.store(...)`
// carries it through unchanged and guests propagate via `?`.
impl From<store::Error> for EntryError {
    fn from(err: store::Error) -> Self {
        Self::Store(err)
    }
}

struct Component;

impl Guest for Component {
    fn run() -> Result<(), EntryError> {
        info("entered run()");

        et::ws_wasi::ws::connect()?;
        let agent_id =
            wait_for_agent_id().ok_or_else(|| EntryError::Runtime("did not receive agent_id".to_string()))?;
        info(&format!("websocket connected with agent_id={agent_id}"));

        let bucket = store::open(&agent_id)?;

        let test_content = format!("Hello from wasi-data1, agent={agent_id}!").into_bytes();
        info(&format!("storing {} bytes to key {FILENAME}", test_content.len()));
        bucket.set(FILENAME, &test_content)?;

        info(&format!("fetching key {FILENAME}"));
        let fetched = bucket
            .get(FILENAME)?
            .ok_or_else(|| EntryError::Runtime(format!("bucket.get({FILENAME}) returned none after set")))?;

        if fetched != test_content {
            return Err(EntryError::Runtime(format!(
                "data mismatch: sent {} bytes, got {} bytes",
                test_content.len(),
                fetched.len()
            )));
        }
        info("VERIFICATION SUCCESS -- keyvalue roundtrip matches");

        et::ws_wasi::ws::disconnect();
        info("workflow complete");
        #[cfg(feature = "coverage")]
        coverage::dump();
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
    let _ready = wasi::io::poll::poll(&[&pollable]);
}

export!(Component);
