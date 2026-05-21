//! Tiny helper traits for the WIT String error boundary.
//!
//! WIT-binding host impls and WASI guest exports almost universally
//! return `Result<_, String>` (it keeps the WIT wire format simple),
//! which means foreign errors need to be funnelled into a String with
//! a bit of call-site context. This module carries the only
//! `.map_err(...)` for that pattern — every consumer uses
//! `.wit_context("ctx")` (or `.wit_context_debug("ctx")` for error
//! types that only implement `Debug`).

extern crate alloc;

use alloc::format;
use alloc::string::String;

/// Extension over any `Result<T, E>` whose error is `Display`. Maps the
/// failure to a `String` carrying `context` plus the inner error.
pub trait WitErrExt<T> {
    fn wit_context(self, context: &str) -> Result<T, String>;
}

impl<T, E: core::fmt::Display> WitErrExt<T> for Result<T, E> {
    fn wit_context(self, context: &str) -> Result<T, String> {
        self.map_err(|err| format!("{context}: {err}"))
    }
}

/// Same as [`WitErrExt::wit_context`] but uses `Debug` formatting — some
/// bindgen-generated WIT error types implement `Debug` but not `Display`.
pub trait WitErrDebugExt<T> {
    fn wit_context_debug(self, context: &str) -> Result<T, String>;
}

impl<T, E: core::fmt::Debug> WitErrDebugExt<T> for Result<T, E> {
    fn wit_context_debug(self, context: &str) -> Result<T, String> {
        self.map_err(|err| format!("{context}: {err:?}"))
    }
}
