//! Wasm 1.0 core-module twin of the math1 `FedAvg` module, which `et-ws-wasi-runner` runs on NASA's `SpaceWasm`.
//!
//! The storage-driven exchange `wasi-math1` runs, through the `et` imports rather than the `et:ws-wasi` world: it
//! waits for the relayed `math1-input` pointer, reads the input it names from storage, runs `FedAvg`, and stores the
//! global model as `math1-output.json` in its own bucket.
//!
//! Built for `wasm32v1-none`, the Wasm 1.0 target `SpaceWasm` runs, so it is `no_std`. The crate is gated on that
//! target, so a host `cargo check --workspace` builds an empty cdylib.

#![cfg(all(target_arch = "wasm32", target_os = "none"))]
#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::ToString as _;

use edge_toolkit::ws::ServerMessage;
use et_core_guest::wasm::Host;
use et_core_guest::{Guest, GuestError};
use et_math1::{InputPointer, Math1Input, OUTPUT_FILENAME, fed_avg, output_json, parse_pointer};

/// How many windows to wait for the pointer in before giving up.
const POINTER_WAITS: u32 = 100;
/// How long each window lasts, short enough that one of the trigger's re-broadcasts lands in it.
const POINTER_WAIT_MS: u32 = 100;

/// Wait for the input pointer, fit the model to the input it names, and store the model in this agent's bucket.
#[expect(
    clippy::single_call_fn,
    reason = "the guest's work, which `export!` makes its run; named apart from the export it is wrapped in"
)]
fn twin(guest: &mut Guest<Host>) -> Result<(), GuestError> {
    let agent_id = guest.start()?;
    guest.info("waiting for the math1-input pointer broadcast");
    let pointer = wait_for_pointer(guest)?;

    guest.info(&format!(
        "reading input from bucket={} key={}",
        pointer.bucket, pointer.filename
    ));
    let input_bytes = guest
        .get(&pointer.bucket, &pointer.filename)?
        .ok_or_else(|| GuestError::Runtime(format!("input {} not found", pointer.filename)))?;
    let input: Math1Input = serde_json::from_slice(&input_bytes)?;

    guest.info(&format!(
        "running FedAvg - {} clients x {} rounds x {} local epochs",
        input.clients.len(),
        input.rounds,
        input.epochs
    ));
    let model = fed_avg(&input);
    guest.info(&format!("global model weight={} bias={}", model.weight, model.bias));

    guest.set(&agent_id, OUTPUT_FILENAME, output_json("core-math1", model).as_bytes())?;
    guest.info(&format!("stored the global model to {OUTPUT_FILENAME}"));

    guest.disconnect();
    guest.info("workflow complete");
    Ok(())
}

/// The relayed `math1-input` pointer, waited for in windows of [`POINTER_WAIT_MS`].
///
/// The trigger re-broadcasts the pointer until the output lands, so each window only has to catch one of them.
#[expect(
    clippy::single_call_fn,
    reason = "the inbox-draining step of twin, named for what it waits on"
)]
fn wait_for_pointer(guest: &mut Guest<Host>) -> Result<InputPointer, GuestError> {
    for _ in 0..POINTER_WAITS {
        if let Some(ServerMessage::RelayText { content }) = guest.recv(POINTER_WAIT_MS)?
            && let Ok(pointer) = parse_pointer(&content)
        {
            return Ok(pointer);
        }
    }
    Err(GuestError::Runtime(
        "did not receive the math1-input pointer".to_string(),
    ))
}

et_core_guest::export!(twin);
