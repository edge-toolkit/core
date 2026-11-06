//! The names and codes of the `et` imports, which the host that provides them and the guest that calls them share.

/// The import module every `et` function is imported from.
pub const MODULE: &str = "et";
/// The export the host calls to run the guest.
pub const RUN: &str = "run";

/// Open the websocket and register with the hub.
pub const CONNECT: &str = "connect";
/// Put the agent id the hub assigned in the result slot.
pub const AGENT_ID: &str = "agent_id";
/// Send one `ClientMessage`.
pub const SEND: &str = "send";
/// Wait for one `ServerMessage`, putting it in the result slot.
pub const RECV: &str = "recv";
/// Copy the result slot into the guest's memory.
pub const TAKE: &str = "take";
/// Close the websocket.
pub const DISCONNECT: &str = "disconnect";
/// Put the value stored under a key in a bucket in the result slot.
pub const KV_GET: &str = "kv_get";
/// Store a value under a key in a bucket.
pub const KV_SET: &str = "kv_set";
/// Record one log entry.
pub const LOG: &str = "log";
/// Every function the module provides, which is all a guest may import from it.
pub const IMPORTS: [&str; 9] = [CONNECT, AGENT_ID, SEND, RECV, TAKE, DISCONNECT, KV_GET, KV_SET, LOG];

/// The call succeeded, or `run` did.
pub const SUCCESS: i32 = 0;
/// `run`: the guest failed for a reason of its own, which it logged.
pub const FAILED: i32 = 1;
/// `recv`: no message arrived within the timeout.
pub const NOTHING: i32 = 0;
/// `kv_get`: the bucket holds nothing under the key.
pub const ABSENT: i32 = 0;
/// `kv_get`: the value is in the result slot.
pub const FOUND: i32 = 1;

/// The websocket failed, or the hub could not be reached.
pub const TRANSPORT: i32 = -1;
/// A message was not the JSON of the message it had to be.
pub const DECODE: i32 = -2;
/// The websocket is not open.
pub const NOT_CONNECTED: i32 = -3;
/// The websocket is already open.
pub const ALREADY_CONNECTED: i32 = -4;
/// The hub's storage refused the request, or failed.
pub const STORE: i32 = -5;

/// `log`: the `wasi:logging` level trace.
pub const LOG_TRACE: i32 = 0;
/// `log`: the `wasi:logging` level debug.
pub const LOG_DEBUG: i32 = 1;
/// `log`: the `wasi:logging` level info.
pub const LOG_INFO: i32 = 2;
/// `log`: the `wasi:logging` level warn.
pub const LOG_WARN: i32 = 3;
/// `log`: the `wasi:logging` level error.
pub const LOG_ERROR: i32 = 4;
/// `log`: the `wasi:logging` level critical.
pub const LOG_CRITICAL: i32 = 5;
