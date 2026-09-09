//! One set of WASI Preview 2 bindings, plus the helpers every ws-module guest needs, shared by all of them.
//!
//! `wit_bindgen::generate!` emits a whole binding tree -- the imported `et:ws-wasi` and `wasi:*` interfaces, the
//! exported `entry` world, and the `export!` macro that wires a guest type to it. Generated per crate, every
//! guest ends up with its own copy of those types, which forces each one to re-declare the same `use` block, the
//! same `From` impls lifting a `ws-error` or a `store-error` into `entry-error`, and the same connect-and-wait
//! preamble -- not because the code differs but because the types do. Generating once here and pointing each
//! guest at this crate (`pub_export_macro` + `default_bindings_module`) makes those types shared, so the
//! boilerplate can be written once too and a guest is left holding only its own workflow.
//!
//! A guest depends on this crate, imports what it needs from it, and finishes with
//! `et_wasi_guest::export!(Component);` in place of the local `export!`.
//!
//! Crate-level cfg gate: the generated extern declarations reference WASI imports that only resolve on
//! `wasm32-wasip2`. Gating the whole module on `target_os = "wasi"` lets the crate sit in the parent workspace
//! -- `cargo check --workspace` from the repo root sees an empty crate for the host target rather than linker
//! errors -- and matches the gate every guest already carries.

#![cfg(target_os = "wasi")]

wit_bindgen::generate!({
    // ET_WIT_DIR is the absolute path to generated/specs/wit, emitted by build.rs.
    path: env!("ET_WIT_DIR"),
    world: "module",
    generate_all,
    // Let a guest crate invoke the generated `export!` and have the expansion resolve its type paths back here
    // rather than in the guest, which is what makes one binding tree serve every guest.
    pub_export_macro: true,
    default_bindings_module: "et_wasi_guest",
});

use std::sync::OnceLock;

use et::ws_wasi::ws::WsError;
use exports::et::ws_wasi::entry::EntryError;
use wasi::keyvalue::store;
use wasi::logging::logging::{self, Level};

/// How many times [`wait_for_agent_id`] polls before giving up.
const AGENT_ID_POLLS: u32 = 100;

/// Gap between those polls.
const AGENT_ID_POLL_INTERVAL_MS: u64 = 50;

/// Nanoseconds per millisecond, for the monotonic clock's duration argument.
const NANOS_PER_MILLI: u64 = 1_000_000;

// Lets `?` lift a `ws-error` into `entry-error.ws(...)` so a guest's `run` body stays free of explicit
// `.map_err`s (which the workspace's no-map-err ast-grep rule bans outside listed error.rs files anyway).
impl From<WsError> for EntryError {
    fn from(err: WsError) -> Self {
        Self::Ws(err)
    }
}

// Same idea for `wasi:keyvalue/store.error` -- the upstream type is a value variant (no resources involved), so
// `entry-error.store(...)` carries it through unchanged and guests propagate via `?`.
impl From<store::Error> for EntryError {
    fn from(err: store::Error) -> Self {
        Self::Store(err)
    }
}

/// The calling guest's log context, set once by [`start`].
///
/// Held here rather than passed to every [`info`] call so a guest's logging reads as `info("...")` with no
/// per-guest wrapper function to define -- which is what stops four guests from each carrying an identical one.
static LOG_CONTEXT: OnceLock<String> = OnceLock::new();

/// Log at info level under the context [`start`] recorded.
///
/// Before `start` has run there is no guest to name, so this falls back to this crate's own name rather than
/// failing: a message logged that early is about the bootstrap, not about the guest's workflow.
pub fn info(message: &str) {
    let context = LOG_CONTEXT.get().map_or(env!("CARGO_PKG_NAME"), String::as_str);
    logging::log(Level::Info, context, message);
}

/// Record `context`, connect to the hub, and return the assigned `agent_id`.
///
/// Every guest opens exactly this way -- announce entry, connect, wait out the `ConnectAck`, log the id -- and
/// the id is not incidental to the rest: it names the guest's own storage bucket. Doing the whole preamble here
/// leaves a guest's `run` starting on its own first real step.
pub fn start(context: &str) -> Result<String, EntryError> {
    let _already_set = LOG_CONTEXT.set(context.to_string());
    info("entered run()");
    et::ws_wasi::ws::connect()?;
    let agent_id = wait_for_agent_id().ok_or_else(|| EntryError::Runtime("did not receive agent_id".to_string()))?;
    info(&format!("websocket connected with agent_id={agent_id}"));
    Ok(agent_id)
}

/// Block the guest for `ms` milliseconds.
///
/// A WASI guest has no thread to sleep, so this subscribes to the monotonic clock and blocks on the pollable.
pub fn sleep_ms(ms: u64) {
    let pollable = wasi::clocks::monotonic_clock::subscribe_duration(ms * NANOS_PER_MILLI);
    let _ready = wasi::io::poll::poll(&[&pollable]);
}

/// Poll `agent_id` until the server's `ConnectAck` has landed, returning `None` if it never does.
///
/// `ws.connect` waits briefly for that message, but the host returns once its wait expires regardless, so
/// polling is what keeps this safe under load.
#[must_use]
pub fn wait_for_agent_id() -> Option<String> {
    for _ in 0..AGENT_ID_POLLS {
        let id = et::ws_wasi::ws::agent_id();
        if !id.is_empty() {
            return Some(id);
        }
        sleep_ms(AGENT_ID_POLL_INTERVAL_MS);
    }
    None
}

#[cfg(feature = "coverage")]
mod coverage;

#[cfg(feature = "coverage")]
pub use self::coverage::dump_coverage;
