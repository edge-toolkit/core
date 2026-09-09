//! Rust WASI Preview 2 port of the comm1 workflow module.
//!
//! Browser comm1 (`services/ws-modules/comm1`) waits for a second agent to
//! be connected, then exchanges broadcast and direct messages with it. The
//! integration test only spins up a single runner, so the WASI port instead
//! exercises the message round-trip with the server itself:
//!   1. Connect and capture our `agent_id`.
//!   2. Send `list-agents`, recv a `list-agents-response`, assert the list
//!      contains our `agent_id` (we're at least in our own roster).
//!   3. Send a `broadcast-message` (fire-and-forget when no peer is online).
//!   4. Disconnect cleanly.
//!
//! Messages cross the WIT boundary as typed `ws-message` variants from the
//! generated `et:ws-messages@0.1.0` package; opaque JSON payloads (the
//! `message` field on broadcast/direct messages) round-trip as strings.

// Crate-level cfg gate: wit-bindgen's generated extern declarations only
// resolve on `wasm32-wasip2`. Gating the whole module on `target_os = "wasi"`
// lets the crate sit in the parent workspace -- `cargo check --workspace`
// from the repo root produces an empty cdylib for the host target without
// linker errors.
#![cfg(target_os = "wasi")]

use et_wasi_guest::et::ws_messages::messages::{BroadcastMessagePayload, ClientMessage, ServerMessage};
use et_wasi_guest::et::ws_wasi::ws;
use et_wasi_guest::exports::et::ws_wasi::entry::{EntryError, Guest};
use et_wasi_guest::{info, start};

const LOG_CONTEXT: &str = env!("CARGO_PKG_NAME");
/// Total time we'll wait for a `list-agents-response`.
/// The server replies immediately under normal load, but we leave headroom for the inbox queue.
const LIST_AGENTS_TIMEOUT_MS: u32 = 2_000;

struct Component;

impl Guest for Component {
    async fn run() -> Result<(), EntryError> {
        let agent_id = start(LOG_CONTEXT)?;
        ws::send(&ClientMessage::ListAgents)?;

        let response = wait_for_list_agents_response(LIST_AGENTS_TIMEOUT_MS)
            .ok_or_else(|| EntryError::Runtime("no list-agents-response within timeout".to_string()))?;
        info(&format!(
            "list-agents-response: {} agent(s) registered",
            response.agents.len()
        ));

        let self_listed = response.agents.iter().any(|a| a.agent_id == agent_id);
        if !self_listed {
            return Err(EntryError::Runtime(format!(
                "own agent_id {agent_id} missing from list-agents-response"
            )));
        }
        info("self present in roster");

        let body = serde_json::json!({
            "module": "wasi-comm1",
            "from_agent_id": agent_id,
            "message": "wasi-comm1 broadcast -- likely peerless under the runner test",
        });
        let body_str = match serde_json::to_string(&body) {
            Ok(rendered) => rendered,
            Err(e) => return Err(EntryError::Runtime(format!("serialize broadcast body: {e}"))),
        };
        ws::send(&ClientMessage::BroadcastMessage(BroadcastMessagePayload {
            message: body_str,
        }))?;
        info("broadcast sent");

        ws::disconnect();
        info("workflow complete");
        #[cfg(feature = "coverage")]
        et_wasi_guest::dump_coverage("et_ws_wasi_comm1.profraw");
        Ok(())
    }
}

/// Drain the recv inbox until we see a `list-agents-response`.
/// Each `recv` call blocks for the remaining budget; keep going until either the budget is exhausted or we
/// get the message we want.
fn wait_for_list_agents_response(
    total_timeout_ms: u32,
) -> Option<et_wasi_guest::et::ws_messages::messages::ListAgentsResponsePayload> {
    let mut remaining = total_timeout_ms;
    while remaining > 0 {
        let chunk = remaining.min(200);
        match ws::recv(chunk).ok()? {
            Some(ServerMessage::ListAgentsResponse(payload)) => return Some(payload),
            Some(_) => {}
            None => {}
        }
        remaining = remaining.saturating_sub(chunk);
    }
    None
}

et_wasi_guest::export!(Component);
