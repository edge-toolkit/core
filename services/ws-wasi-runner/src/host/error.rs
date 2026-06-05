//! Error helpers for the runner's host impls. Each trait carries the
//! only `.map_err(...)` for the pattern it covers; every host file
//! reaches the conversions through one of the `.map_tungstenite_err(...)`
//! / `.map_decode_err(...)` / `.kv_context(...)` / `.request_device_err()`
//! / `.wit_context(...)` shorthands.

use crate::bindings::et::ws_wasi::ws::WsError;
use crate::bindings::wasi::keyvalue::store::Error as KvError;
use crate::bindings::wasi::webgpu::webgpu::{RequestDeviceError, RequestDeviceErrorKind};

/// Generic fallback: maps any `Display` error to `Result<_, String>`.
/// Used in spots (e.g. inside `tokio::task::spawn_blocking`) where the
/// scope is small enough that a typed enum would be overkill.
pub trait WitErrExt<T> {
    fn wit_context(self, context: &str) -> Result<T, String>;
}

impl<T, E: std::fmt::Display> WitErrExt<T> for Result<T, E> {
    fn wit_context(self, context: &str) -> Result<T, String> {
        self.map_err(|err| format!("{context}: {err}"))
    }
}

/// Maps any `Display` error into `wasi:keyvalue/store`'s
/// `error.other(message)` variant.
pub trait KvErrExt<T> {
    fn kv_context(self, context: &str) -> Result<T, KvError>;
}

impl<T, E: std::fmt::Display> KvErrExt<T> for Result<T, E> {
    fn kv_context(self, context: &str) -> Result<T, KvError> {
        self.map_err(|err| KvError::Other(format!("{context}: {err}")))
    }
}

/// Build a `wasi:keyvalue/store.error.other("<op> not implemented")` -- the
/// closest thing the WIT-spec enum has to a `NotImplemented` variant.
#[must_use]
pub fn kv_not_implemented(operation: &str) -> KvError {
    KvError::Other(format!("{operation} not implemented"))
}

/// Maps any `Display` error into `wasi:webgpu/webgpu`'s
/// `request-device-error.operation-error` variant.
pub trait RequestDeviceErrExt<T> {
    fn request_device_err(self) -> Result<T, RequestDeviceError>;
}

impl<T, E: std::fmt::Display> RequestDeviceErrExt<T> for Result<T, E> {
    fn request_device_err(self) -> Result<T, RequestDeviceError> {
        self.map_err(|err| RequestDeviceError {
            kind: RequestDeviceErrorKind::OperationError,
            message: format!("{err}"),
        })
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
///
/// The context string is prefixed onto the source's `Display` rendering so
/// the carried message reads as `"ws <context>: <source>"`.
pub trait WsTransportErrExt<T> {
    fn map_tungstenite_err(self, context: &str) -> Result<T, WsError>;
}

impl<T> WsTransportErrExt<T> for Result<T, tokio_tungstenite::tungstenite::Error> {
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "tungstenite::Error is non_exhaustive; the wildcard absorbs every non-ConnectionClosed variant"
    )]
    fn map_tungstenite_err(self, context: &str) -> Result<T, WsError> {
        use tokio_tungstenite::tungstenite::Error as Tungstenite;
        self.map_err(|err| match &err {
            Tungstenite::ConnectionClosed | Tungstenite::AlreadyClosed => WsError::NotConnected,
            _ => WsError::Transport(format!("ws {context}: {err}")),
        })
    }
}

/// Maps a JSON serialize / deserialize error into `WsError::Decode`.
///
/// Generic over `Display` so plain `serde_json::Error` (from
/// `to_string`) and `serde_path_to_error::Error<serde_json::Error>`
/// (from path-tracking `deserialize`) both flow through the same
/// `?`-friendly conversion.
pub trait WsDecodeErrExt<T> {
    fn map_decode_err(self, context: &str) -> Result<T, WsError>;
}

impl<T, E: std::fmt::Display> WsDecodeErrExt<T> for Result<T, E> {
    fn map_decode_err(self, context: &str) -> Result<T, WsError> {
        self.map_err(|err| WsError::Decode(format!("{context}: {err}")))
    }
}
