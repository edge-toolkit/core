//! Error conversions for the runner's std host impls: from its HTTP client and its websocket.

use crate::bindings::et::ws_wasi::ws::WsError;
use crate::bindings::wasi::keyvalue::store::Error as KvError;

/// Conversions into `wasi:keyvalue/store`'s `error.other(message)` variant.
///
/// One impl per concrete source type: the REST client's byte stream and its request errors are what the
/// `keyvalue` host impls can fail on besides the resource table.
impl From<reqwest::Error> for KvError {
    fn from(err: reqwest::Error) -> Self {
        Self::Other(err.to_string())
    }
}

impl From<et_rest_client::Error<()>> for KvError {
    fn from(err: et_rest_client::Error<()>) -> Self {
        Self::Other(err.to_string())
    }
}

/// Maps a `tokio_tungstenite::tungstenite::Error` into a typed `WsError`.
///
/// Specialised to `tungstenite::Error` so the variant carries through:
///   - `ConnectionClosed` / `AlreadyClosed` become `WsError::NotConnected`,
///     letting guests use the typed not-connected case for reconnect logic
///     instead of pattern-matching on a `Transport(String)`.
///   - Everything else (IO, TLS, URL, HTTP-upgrade, write-buffer-full,
///     Capacity, Protocol, `AttackAttempt`, Utf8) is transport-level -- the
///     wire never delivered cleanly -- and lands in `WsError::Transport`.
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "tungstenite::Error is non_exhaustive; the wildcard absorbs every non-ConnectionClosed variant"
)]
impl From<tokio_tungstenite::tungstenite::Error> for WsError {
    fn from(err: tokio_tungstenite::tungstenite::Error) -> Self {
        use tokio_tungstenite::tungstenite::Error as Tungstenite;
        match &err {
            Tungstenite::ConnectionClosed | Tungstenite::AlreadyClosed => Self::NotConnected,
            _ => Self::Transport(err.to_string()),
        }
    }
}

impl From<et_ws_runner_common::ConnectError> for WsError {
    fn from(err: et_ws_runner_common::ConnectError) -> Self {
        Self::Transport(err.to_string())
    }
}
