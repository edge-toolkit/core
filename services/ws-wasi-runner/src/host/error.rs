//! Error helpers for the runner's host impls. Each trait carries the
//! only `.map_err(...)` for the pattern it covers; every host file
//! reaches the conversions through one of the `.ws_transport(...)` /
//! `.ws_protocol(...)` / `.kv_context(...)` / `.request_device_err()`
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

/// Build a `wasi:keyvalue/store.error.other("<op> not implemented")` — the
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

/// Maps a foreign transport error (tcp, tls, websocket frame) into the
/// `transport: ...` string form of `et:ws-wasi/ws.ws-error` (which the WIT
/// declares as a `string` alias).
pub trait WsTransportErrExt<T> {
    fn ws_transport(self, context: &str) -> Result<T, WsError>;
}

impl<T, E: std::fmt::Display> WsTransportErrExt<T> for Result<T, E> {
    fn ws_transport(self, context: &str) -> Result<T, WsError> {
        self.map_err(|err| format!("transport: {context}: {err}"))
    }
}

/// Maps a foreign serialization / deserialization error into the
/// `protocol: ...` string form of `et:ws-wasi/ws.ws-error`.
pub trait WsProtocolErrExt<T> {
    fn ws_protocol(self, context: &str) -> Result<T, WsError>;
}

impl<T, E: std::fmt::Display> WsProtocolErrExt<T> for Result<T, E> {
    fn ws_protocol(self, context: &str) -> Result<T, WsError> {
        self.map_err(|err| format!("protocol: {context}: {err}"))
    }
}
