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
// wit_bindgen::generate! emits `unsafe fn` and `#[export_name]` items;
// `export!(Component)` does the same. Both trip workspace
// `unsafe_code = "deny"` lint; expect it at crate scope because outer
// `#[expect]` on the macro invocations themselves doesn't propagate to
// the items they expand into.
#![expect(unsafe_code)]

wit_bindgen::generate!({
    path: "../../../generated/specs/wit",
    world: "module",
    generate_all,
});

use et::ws_messages::messages::{BroadcastMessagePayload, ClientMessage, ServerMessage};
use et::ws_wasi::ws::WsError;
use exports::et::ws_wasi::entry::{EntryError, Guest};
use wasi::logging::logging::{self, Level};

const LOG_CONTEXT: &str = env!("CARGO_PKG_NAME");
/// Total time we'll wait for a `list-agents-response`. The server replies
/// immediately under normal load, but we leave headroom for the inbox queue.
const LIST_AGENTS_TIMEOUT_MS: u32 = 2_000;

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

struct Component;

impl Guest for Component {
    fn run() -> Result<(), EntryError> {
        info("entered run()");

        et::ws_wasi::ws::connect()?;
        let agent_id =
            wait_for_agent_id().ok_or_else(|| EntryError::Runtime("did not receive agent_id".to_string()))?;
        info(&format!("websocket connected with agent_id={agent_id}"));

        et::ws_wasi::ws::send(&ClientMessage::ListAgents)?;

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
        et::ws_wasi::ws::send(&ClientMessage::BroadcastMessage(BroadcastMessagePayload {
            message: body_str,
        }))?;
        info("broadcast sent");

        et::ws_wasi::ws::disconnect();
        info("workflow complete");
        Ok(())
    }
}

/// Drain the recv inbox until we see a `list-agents-response`. Each `recv`
/// call blocks for the remaining budget; keep going until either the budget
/// is exhausted or we get the message we want.
fn wait_for_list_agents_response(
    total_timeout_ms: u32,
) -> Option<et::ws_messages::messages::ListAgentsResponsePayload> {
    let mut remaining = total_timeout_ms;
    while remaining > 0 {
        let chunk = remaining.min(200);
        match et::ws_wasi::ws::recv(chunk).ok()? {
            Some(ServerMessage::ListAgentsResponse(payload)) => return Some(payload),
            Some(_) => {}
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
    drop(wasi::io::poll::poll(&[&pollable]));
}

export!(Component);
