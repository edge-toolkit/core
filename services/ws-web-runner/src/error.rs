//! Error types + helpers for the embedded Deno runtime.
//!
//! `RunnerError` is the public error returned by `run_module`. `JsErrExt`
//! is an extension trait that wraps the only `.map_err(...)` calls in the
//! crate -- it converts foreign error types into `deno_error::JsErrorBox`
//! so call sites can use `.js_err()` / `.js_err_context(...)` instead of
//! hand-rolling closures.

use thiserror::Error;

/// Errors that `run_module` can produce.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RunnerError {
    #[error(transparent)]
    Common(#[from] et_ws_runner_common::BootstrapError),

    #[error("deno runtime error: {0}")]
    Deno(#[from] deno_core::error::CoreError),

    #[error("deno runtime error: {0}")]
    DenoGeneric(#[from] deno_core::error::AnyError),
}

/// Maps any `Display` error into a generic `JsErrorBox`.
///
/// Per repo naming convention (see CLAUDE.md), these are `map_*` because they
/// are custom-`map_err` wrappers -- the name signals "this calls `map_err`
/// under the hood, just hiding the closure."
pub trait JsErrExt<T> {
    /// Convert the error via `Display` into a generic JS error.
    fn map_js_err(self) -> Result<T, deno_error::JsErrorBox>;

    /// Convert the error via `Display` with additional context.
    fn map_js_err_with_context(self, context: impl FnOnce() -> String) -> Result<T, deno_error::JsErrorBox>;
}

impl<T, E: std::fmt::Display> JsErrExt<T> for Result<T, E> {
    fn map_js_err(self) -> Result<T, deno_error::JsErrorBox> {
        self.map_err(|err| deno_error::JsErrorBox::generic(err.to_string()))
    }

    fn map_js_err_with_context(self, context: impl FnOnce() -> String) -> Result<T, deno_error::JsErrorBox> {
        self.map_err(|err| deno_error::JsErrorBox::generic(format!("{}: {err}", context())))
    }
}
