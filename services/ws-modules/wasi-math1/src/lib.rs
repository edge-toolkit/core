//! Rust WASI Preview 2 twin of the math1 `FedAvg` module.
//!
//! Storage-driven, under wasmtime -- the family's only native (non-browser) executor: waits for the
//! broadcast `math1-input` pointer (relayed to the guest through `ws.recv`), reads the input JSON
//! (client datasets + hyperparameters) from storage via `wasi:keyvalue/store`, runs the kernel --
//! only `+ - * /` on f64 in a fixed evaluation order, bit-identical to the browser twins -- and
//! stores the global model to `math1-output.json` in its own bucket, where the test harness reads
//! and verifies it.
//!
//! Crate-level cfg gate: the wit-bindgen-generated extern declarations
//! reference WASI imports that only resolve on `wasm32-wasip2`. Gating the
//! whole module on `target_os = "wasi"` lets the crate sit in the parent
//! workspace -- `cargo check --workspace` from the repo root produces an
//! empty cdylib for the host target without linker errors.

#![cfg(target_os = "wasi")]
#![expect(clippy::float_arithmetic, reason = "the FedAvg kernel is float math by design")]

use et_wasi_guest::et::ws_messages::messages::ServerMessage;
use et_wasi_guest::et::ws_wasi::ws;
use et_wasi_guest::exports::et::ws_wasi::entry::{EntryError, Guest};
use et_wasi_guest::wasi::keyvalue::store;
use et_wasi_guest::{info, start};
use serde::Deserialize;

const LOG_CONTEXT: &str = env!("CARGO_PKG_NAME");

/// The canonical input: per-client (feature, target) samples plus the training hyperparameters.
#[derive(Deserialize)]
struct Math1Input {
    clients: Vec<Vec<(f64, f64)>>,
    rounds: u32,
    epochs: u32,
    learning_rate: f64,
}

/// The broadcast pointer naming the storage bucket + filename the input JSON was injected at.
#[derive(Deserialize)]
struct InputPointer {
    bucket: String,
    filename: String,
}

/// Sample count as f64, accumulated additively to avoid an integer-to-float cast.
fn sample_count(samples: &[(f64, f64)]) -> f64 {
    samples.iter().fold(0.0_f64, |count, _| count + 1.0)
}

/// Runs the `FedAvg` simulation on `input` and returns the final global (weight, bias).
#[expect(
    clippy::single_call_fn,
    reason = "the kernel is a distinct step, kept separate from the ws workflow"
)]
fn fed_avg(input: &Math1Input) -> (f64, f64) {
    let mut weight = 0.0_f64;
    let mut bias = 0.0_f64;
    let total_samples: f64 = input
        .clients
        .iter()
        .fold(0.0_f64, |acc, samples| acc + sample_count(samples));
    for _ in 0_u32..input.rounds {
        let mut merged_weight = 0.0_f64;
        let mut merged_bias = 0.0_f64;
        for samples in &input.clients {
            let count = sample_count(samples);
            let mut client_weight = weight;
            let mut client_bias = bias;
            for _ in 0_u32..input.epochs {
                let mut grad_weight = 0.0_f64;
                let mut grad_bias = 0.0_f64;
                for &(feature, target) in samples {
                    let residual = client_weight * feature + client_bias - target;
                    grad_weight += residual * feature;
                    grad_bias += residual;
                }
                client_weight -= input.learning_rate * (2.0 * grad_weight / count);
                client_bias -= input.learning_rate * (2.0 * grad_bias / count);
            }
            merged_weight += client_weight * count;
            merged_bias += client_bias * count;
        }
        weight = merged_weight / total_samples;
        bias = merged_bias / total_samples;
    }
    (weight, bias)
}

struct Component;

impl Guest for Component {
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
        let (weight, bias) = fed_avg(&input);
        info(&format!("global model weight={weight} bias={bias}"));

        let own_bucket = store::open(&agent_id)?;
        let output = serde_json::json!({ "module": "wasi-math1", "weight": weight, "bias": bias }).to_string();
        own_bucket.set("math1-output.json", output.as_bytes())?;
        info("stored the global model to math1-output.json");

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
fn wait_for_pointer() -> Option<InputPointer> {
    for _ in 0..100 {
        if let Ok(Some(ServerMessage::RelayText(payload))) = ws::recv(100)
            && let Ok(json) = serde_json::from_str::<serde_json::Value>(&payload.content)
            && json.get("type").and_then(serde_json::Value::as_str) == Some("math1-input")
            && let Ok(pointer) = serde_json::from_value::<InputPointer>(json)
        {
            return Some(pointer);
        }
    }
    None
}

et_wasi_guest::export!(Component);
