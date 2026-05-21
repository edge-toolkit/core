//! Bindgen-specific error helpers for the runner's host impls.
//!
//! The generic `WitErrExt` (which maps any `Display` error to
//! `Result<_, String>`) lives in the shared `et-wit-err` crate so the
//! WASI guest modules can use the same trait without duplicating it.
//! This file is just the home of the helpers that reference the runner's
//! own bindgen-generated WIT error types — those types can't be reached
//! from a leaf workspace crate without pulling in the runner itself.

use crate::bindings::wasi::keyvalue::store::Error as KvError;
use crate::bindings::wasi::webgpu::webgpu::{RequestDeviceError, RequestDeviceErrorKind};

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
