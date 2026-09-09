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

use et_wasi_guest::et::ws_wasi::ws;
use et_wasi_guest::exports::et::ws_wasi::entry::{EntryError, Guest};
use et_wasi_guest::wasi::keyvalue::store;
use et_wasi_guest::{info, start};

const LOG_CONTEXT: &str = env!("CARGO_PKG_NAME");
const FILENAME: &str = "test_data.txt";

struct Component;

impl Guest for Component {
    async fn run() -> Result<(), EntryError> {
        let agent_id = start(LOG_CONTEXT)?;
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

        ws::disconnect();
        info("workflow complete");
        #[cfg(feature = "coverage")]
        et_wasi_guest::dump_coverage("et_ws_wasi_data1.profraw");
        Ok(())
    }
}

et_wasi_guest::export!(Component);
