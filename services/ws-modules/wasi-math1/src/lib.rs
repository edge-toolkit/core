//! Rust WASI Preview 2 twin of the math1 `FedAvg` module.
//!
//! Storage-driven, under wasmtime -- the family's only native (non-browser) executor: waits for the
//! broadcast `math1-input` pointer (relayed to the guest through `ws.recv`), reads the input JSON
//! (client datasets + hyperparameters) from storage via `wasi:keyvalue/store`, runs the kernel --
//! only `+ - * /` on f64 in a fixed evaluation order, bit-identical to the browser twins -- and
//! stores the global model to `math1-output.json` in its own bucket, where the harness reads
//! and verifies it.
//!
//! Crate-level cfg gate: the wit-bindgen-generated extern declarations
//! reference WASI imports that only resolve on `wasm32-wasip2`. Gating the
//! whole module on `target_os = "wasi"` lets the crate sit in the parent
//! workspace -- `cargo check --workspace` from the repo root produces an
//! empty cdylib for the host target without linker errors.

#![cfg(target_os = "wasi")]
#![cfg_attr(feature = "docs", doc = "## Feature flags")]
#![cfg_attr(feature = "docs", doc = document_features::document_features!())]

use et_math1::{InputPointer, Math1Input, OUTPUT_FILENAME, fed_avg, output_json, parse_pointer};
use et_wasi_guest::et::ws_messages::messages::ServerMessage;
use et_wasi_guest::et::ws_wasi::ws;
use et_wasi_guest::exports::et::ws_wasi::entry::{EntryError, Guest};
use et_wasi_guest::wasi::keyvalue::store;
use et_wasi_guest::{info, start};

const LOG_CONTEXT: &str = env!("CARGO_PKG_NAME");

struct Component;

impl Guest for Component {
    /// Wait for the input pointer, fit the model to the input it names, and store the model in this agent's bucket.
    #[expect(
        clippy::unused_async_trait_impl,
        reason = "entry.run is an async export of the WIT world, so the generated trait fixes the signature"
    )]
    async fn run() -> Result<(), EntryError> {
        let agent_id = start(LOG_CONTEXT)?;
        info("waiting for the math1-input pointer broadcast");
        let pointer = wait_for_pointer()
            .ok_or_else(|| EntryError::Runtime("did not receive the math1-input pointer".to_string()))?;

        info(&format!(
            "reading input from bucket={} key={}",
            pointer.bucket, pointer.filename
        ));
        let input_bucket = store::open(&pointer.bucket)?;
        let input_bytes = input_bucket
            .get(&pointer.filename)?
            .ok_or_else(|| EntryError::Runtime(format!("input {} not found", pointer.filename)))?;
        let input: Math1Input = match serde_json::from_slice(&input_bytes) {
            Ok(input) => input,
            Err(err) => return Err(EntryError::Runtime(format!("input JSON parse failed: {err}"))),
        };

        info(&format!(
            "running FedAvg - {} clients x {} rounds x {} local epochs",
            input.clients.len(),
            input.rounds,
            input.epochs
        ));
        let model = fed_avg(&input);
        info(&format!("global model weight={} bias={}", model.weight, model.bias));

        let own_bucket = store::open(&agent_id)?;
        own_bucket.set(OUTPUT_FILENAME, output_json("wasi-math1", model).as_bytes())?;
        info(&format!("stored the global model to {OUTPUT_FILENAME}"));

        ws::disconnect();
        info("workflow complete");
        #[cfg(feature = "coverage")]
        et_wasi_guest::dump_coverage("et_ws_wasi_math1.profraw");
        Ok(())
    }
}

/// Drain the recv inbox until the relayed `math1-input` pointer broadcast arrives.
///
/// The fake agent re-broadcasts the pointer until the output lands, so each 100ms recv window only
/// has to catch one of them; foreign frames arrive as `relay-text` envelopes.
#[expect(
    clippy::single_call_fn,
    reason = "the inbox-draining step of run, named for what it waits on"
)]
fn wait_for_pointer() -> Option<InputPointer> {
    for _ in 0_u32..100 {
        if let Ok(Some(ServerMessage::RelayText(payload))) = ws::recv(100)
            && let Ok(pointer) = parse_pointer(&payload.content)
        {
            return Some(pointer);
        }
    }
    None
}

et_wasi_guest::export!(Component);
