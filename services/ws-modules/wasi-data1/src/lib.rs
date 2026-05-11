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
//! own `client_id`, which maps host-side to `/storage/{agent_id}/{key}` —
//! same backend store, same auth boundary (writes only succeed inside
//! one's own bucket), one fewer protocol hop.

wit_bindgen::generate!({
    path: "../../ws-wasi-runner/wit",
    world: "module",
    generate_all,
});

use exports::et::ws_wasi::entry::Guest;
use wasi::keyvalue::store;
use wasi::logging::logging::{self, Level};

const LOG_CONTEXT: &str = "wasi-data1";
const FILENAME: &str = "test_data.txt";

fn info(message: &str) {
    logging::log(Level::Info, LOG_CONTEXT, message);
}

struct Component;

impl Guest for Component {
    fn run() -> Result<(), String> {
        info("entered run()");

        et::ws_wasi::ws::connect().map_err(|e| format!("ws connect failed: {e}"))?;
        let agent_id = wait_for_client_id().ok_or_else(|| "did not receive agent_id".to_string())?;
        info(&format!("websocket connected with agent_id={agent_id}"));

        let bucket = store::open(&agent_id).map_err(|e| format!("store.open({agent_id}): {e:?}"))?;

        let test_content = format!("Hello from wasi-data1, agent={agent_id}!").into_bytes();
        info(&format!("storing {} bytes to key {FILENAME}", test_content.len()));
        bucket
            .set(FILENAME, &test_content)
            .map_err(|e| format!("bucket.set({FILENAME}): {e:?}"))?;

        info(&format!("fetching key {FILENAME}"));
        let fetched = bucket
            .get(FILENAME)
            .map_err(|e| format!("bucket.get({FILENAME}): {e:?}"))?
            .ok_or_else(|| format!("bucket.get({FILENAME}) returned none after set"))?;

        if fetched != test_content {
            return Err(format!(
                "data mismatch: sent {} bytes, got {} bytes",
                test_content.len(),
                fetched.len()
            ));
        }
        info("VERIFICATION SUCCESS — keyvalue roundtrip matches");

        et::ws_wasi::ws::disconnect();
        info("workflow complete");
        Ok(())
    }
}

/// `ws.connect` waits briefly for the `ConnectAck` server message, but the
/// host returns once that wait expires regardless. Poll `client_id` to be
/// safe under load.
fn wait_for_client_id() -> Option<String> {
    for _ in 0..100 {
        let id = et::ws_wasi::ws::client_id();
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
