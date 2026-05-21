//! Rust WASI Preview 2 port of the comm1 workflow module.
//!
//! Browser comm1 (`services/ws-modules/comm1`) waits for a second agent to
//! be connected, then exchanges broadcast and direct messages with it. The
//! integration test only spins up a single runner, so the WASI port instead
//! exercises the message round-trip with the server itself:
//!   1. Connect and capture our agent_id.
//!   2. Send `list_agents`, recv a `list_agents_response`, assert the list
//!      contains our agent_id (we're at least in our own roster).
//!   3. Send a `broadcast_message` (fire-and-forget when no peer is online).
//!   4. Disconnect cleanly.
//!
//! Wire-format messages are built with `serde_json::json!` and serialised
//! before going through `ws.send-text`; recv'd frames are parsed with
//! `serde_json::Value`. This mirrors the WsMessage enum in
//! `libs/edge-toolkit/src/ws.rs` but avoids depending on that crate (its
//! transitive deps don't all compile to wasm32-wasip2).

// Crate-level cfg gate: wit-bindgen's generated extern declarations only
// resolve on `wasm32-wasip2`. Gating the whole module on `target_os = "wasi"`
// lets the crate sit in the parent workspace — `cargo check --workspace`
// from the repo root produces an empty cdylib for the host target without
// linker errors.
#![cfg(target_os = "wasi")]

wit_bindgen::generate!({
    path: "../../ws-wasi-runner/wit",
    world: "module",
    generate_all,
});

use et::ws_wasi::ws::WsError;
use exports::et::ws_wasi::entry::{Guest, RunError};
use serde_json::{Value, json};
use wasi::logging::logging::{self, Level};

const LOG_CONTEXT: &str = env!("CARGO_PKG_NAME");
/// Total time we'll wait for a `list_agents_response`. The server replies
/// immediately under normal load, but we leave headroom for the inbox queue.
const LIST_AGENTS_TIMEOUT_MS: u32 = 2_000;

fn info(message: &str) {
    logging::log(Level::Info, LOG_CONTEXT, message);
}

// Flatten the typed host-import error into `RunError::Ws(String)`.
// Plain `From` rather than thiserror's `#[from]` because the
// bindgen-generated `WsError` doesn't impl `Error`.
impl From<WsError> for RunError {
    fn from(source: WsError) -> Self {
        RunError::Ws(format!("{source:?}"))
    }
}

struct Component;

impl Guest for Component {
    fn run() -> Result<(), RunError> {
        info("entered run()");

        et::ws_wasi::ws::connect()?;
        let agent_id = wait_for_agent_id().ok_or_else(|| RunError::Precondition("did not receive agent_id".into()))?;
        info(&format!("websocket connected with agent_id={agent_id}"));

        send_message(&json!({ "type": "list_agents" }))?;

        let response = wait_for_message_kind("list_agents_response", LIST_AGENTS_TIMEOUT_MS)
            .ok_or_else(|| RunError::Precondition("no list_agents_response within timeout".into()))?;
        let agents = response
            .get("agents")
            .and_then(Value::as_array)
            .ok_or_else(|| RunError::Precondition("list_agents_response missing `agents` array".into()))?;
        info(&format!("list_agents_response: {} agent(s) registered", agents.len()));

        let self_listed = agents
            .iter()
            .any(|a| a.get("agent_id").and_then(Value::as_str) == Some(agent_id.as_str()));
        if !self_listed {
            return Err(RunError::Precondition(format!(
                "own agent_id {agent_id} missing from list_agents_response"
            )));
        }
        info("self present in roster");

        send_message(&json!({
            "type": "broadcast_message",
            "message": {
                "module": "wasi-comm1",
                "from_agent_id": agent_id,
                "message": "wasi-comm1 broadcast — likely peerless under the runner test",
            }
        }))?;
        info("broadcast sent");

        et::ws_wasi::ws::disconnect();
        info("workflow complete");
        Ok(())
    }
}

fn send_message(value: &Value) -> Result<(), RunError> {
    // `Value::to_string` is infallible (uses `Display`) — `serde_json::to_string`
    // would only fail on cases `Value` can't represent: non-string map keys,
    // non-finite floats, or self-recursion. The two `json!()` literals in
    // this file hit none of them.
    et::ws_wasi::ws::send_text(&value.to_string())?;
    Ok(())
}

/// Drain the recv inbox until we see a message whose `type` matches `kind`.
/// Each `recv` call blocks for the remaining budget; we keep going until
/// either the budget is exhausted or the inbox runs dry.
fn wait_for_message_kind(kind: &str, total_timeout_ms: u32) -> Option<Value> {
    let mut remaining = total_timeout_ms;
    while remaining > 0 {
        let chunk = remaining.min(200);
        match et::ws_wasi::ws::recv(chunk).ok()? {
            Some(text) => {
                if let Ok(value) = serde_json::from_str::<Value>(&text)
                    && value.get("type").and_then(Value::as_str) == Some(kind)
                {
                    return Some(value);
                }
            }
            None => {}
        }
        remaining = remaining.saturating_sub(chunk);
    }
    None
}

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
