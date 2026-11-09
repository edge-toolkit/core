//! Error conversions for the runner's std host impls: from its HTTP client, its websocket and its serial ports.

use crate::bindings::et::ws_wasi::ws::WsError;
#[cfg(feature = "lidar")]
use crate::bindings::serial_host::et::ws_wasi::serial::SerialError;
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

/// Conversions into `et:ws-wasi/serial`'s `serial-error.io(message)` variant, one per source a port call can fail on.
#[cfg(feature = "lidar")]
impl From<std::io::Error> for SerialError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

#[cfg(feature = "lidar")]
impl From<serialport::Error> for SerialError {
    fn from(err: serialport::Error) -> Self {
        Self::Io(err.to_string())
    }
}

#[cfg(feature = "lidar")]
impl From<wasmtime::component::ResourceTableError> for SerialError {
    fn from(err: wasmtime::component::ResourceTableError) -> Self {
        Self::Io(err.to_string())
    }
}

#[cfg(feature = "lidar")]
impl From<tokio::task::JoinError> for SerialError {
    fn from(err: tokio::task::JoinError) -> Self {
        Self::Io(err.to_string())
    }
}
