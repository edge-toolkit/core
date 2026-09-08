//! WASI Preview 2 trigger for the math1 family: the native counterpart of the browser `math1-sender`.
//!
//! Plays the fake-agent side of the math1 storage exchange under wasmtime: uploads the canonical input JSON
//! (embedded at build time from ws-test-server's `data/math1-input.json`, the same bytes every test harness
//! injects) into this agent's own storage bucket, then broadcasts the `math1-input` pointer once a second so
//! math1 twins pick it up, compute, and store their models. This module only sends; each twin's stored
//! `math1-output.json` is the harness's (or the operator's) to inspect.
//!
//! It exists because the browser sender cannot leave the web runner, and every math1 scenario needs a trigger.
//! With only that one, a scenario pairing a WASI or pyo3 twin still had to start a web runner alongside it just
//! to be triggered -- which drags in `et-ws-web-runner`, the one crate the Windows lane excludes, and made
//! deployments that are otherwise Windows-clean untestable there.
//!
//! The pointer goes out as `client-message.relay-text`, not `broadcast-message`. The twins match a raw
//! unrecognised text frame relayed verbatim, and `broadcast-message` would wrap it in an `et-agent-message`
//! envelope that no twin looks inside -- so the wrong one is silently ignored rather than rejected.
//!
//! Crate-level cfg gate: the bindings this uses reference WASI imports that only resolve on `wasm32-wasip2`.
//! Gating the whole module on `target_os = "wasi"` lets the crate sit in the parent workspace -- `cargo check
//! --workspace` from the repo root produces an empty cdylib for the host target without linker errors.

#![cfg(target_os = "wasi")]

use et_wasi_guest::et::ws_messages::messages::{ClientMessage, RelayTextPayload};
use et_wasi_guest::et::ws_wasi::ws;
use et_wasi_guest::exports::et::ws_wasi::entry::{EntryError, Guest};
use et_wasi_guest::wasi::keyvalue::store;
use et_wasi_guest::{info, sleep_ms, start};

const LOG_CONTEXT: &str = env!("CARGO_PKG_NAME");

/// The canonical input bytes, embedded from the committed file so there is one source of truth.
const MATH1_INPUT_JSON: &str = include_str!(env!("ET_MATH1_INPUT_PATH"));

/// Storage object name the twins' broadcast pointer names, matching the test harnesses.
const INPUT_FILENAME: &str = "math1-input.json";

/// How many one-second-spaced pointer broadcasts to send before completing.
///
/// Matches the browser sender's window. A twin only has to catch one of them, so the count is really a bound on
/// how late a twin may start: a WASI twin fetching its component, or a pyo3 runner embedding an interpreter,
/// can take a while to reach its first `recv` on a cold CI runner.
const BROADCASTS: u32 = 60;

/// Gap between broadcasts.
const BROADCAST_INTERVAL_MS: u64 = 1_000;

struct Component;

impl Guest for Component {
    async fn run() -> Result<(), EntryError> {
        let agent_id = start(LOG_CONTEXT)?;
        let own_bucket = store::open(&agent_id)?;
        own_bucket.set(INPUT_FILENAME, MATH1_INPUT_JSON.as_bytes())?;
        info(&format!("uploaded {INPUT_FILENAME} to bucket={agent_id}"));

        let pointer = serde_json::json!({
            "type": "math1-input",
            "bucket": agent_id,
            "filename": INPUT_FILENAME,
        })
        .to_string();

        info(&format!(
            "broadcasting the math1-input pointer every {BROADCAST_INTERVAL_MS}ms, {BROADCASTS} times"
        ));
        for round in 1_u32..=BROADCASTS {
            ws::send(&ClientMessage::RelayText(RelayTextPayload {
                content: pointer.clone(),
            }))?;
            info(&format!("broadcast {round}/{BROADCASTS}"));
            sleep_ms(BROADCAST_INTERVAL_MS);
        }

        ws::disconnect();
        info("workflow complete");
        Ok(())
    }
}

et_wasi_guest::export!(Component);
