//! What a core module's `et` imports are carried out on.

#[cfg(not(feature = "std"))]
use alloc::{string::String, vec::Vec};

use edge_toolkit::ws::{ClientMessage, ServerMessage};

use crate::bindings::et::ws_wasi::ws::WsError;
use crate::bindings::wasi::keyvalue::store::Error as StoreError;
use crate::bindings::wasi::logging::logging::Level;

/// The hub and its storage as a core module reaches them, and where its log goes.
///
/// These are the operations of the `et:ws-wasi` world a component imports, taking and giving the hub's own message
/// types rather than the world's, since a core module's messages cross as their JSON. A host's `HostState` carries
/// them out as it does a component's.
pub trait CoreHost {
    /// Open the websocket and register with the hub.
    fn connect(&mut self) -> impl Future<Output = Result<(), WsError>>;

    /// The agent id the hub assigned, empty while the websocket is not open.
    fn agent_id(&mut self) -> impl Future<Output = String>;

    /// Send `message` to the hub.
    fn send(&mut self, message: ClientMessage) -> impl Future<Output = Result<(), WsError>>;

    /// The next message from the hub, waiting up to `timeout_ms`; `None` when none arrived in that time.
    fn recv(&mut self, timeout_ms: u32) -> impl Future<Output = Result<Option<ServerMessage>, WsError>>;

    /// Close the websocket.
    fn disconnect(&mut self) -> impl Future<Output = ()>;

    /// The value stored under `key` in the bucket named `bucket`; `None` when it holds nothing under that key.
    fn get(&mut self, bucket: &str, key: &str) -> impl Future<Output = Result<Option<Vec<u8>>, StoreError>>;

    /// Store `value` under `key` in the bucket named `bucket`, replacing what was there.
    fn set(&mut self, bucket: &str, key: &str, value: Vec<u8>) -> impl Future<Output = Result<(), StoreError>>;

    /// Record one log entry from the guest.
    fn log(&mut self, level: Level, context: &str, message: &str) -> impl Future<Output = ()>;
}
