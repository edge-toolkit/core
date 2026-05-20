//! Error type for `run_module`, plus the extension traits that carry the
//! only `map_err` callsites for the runner-level error variants.
//!
//! `reqwest::Error` and `wasmtime::Error` already carry enough context to be
//! useful on their own, so they're forwarded transparently. Guest failures
//! arrive as `String` because the `entry.run` WIT signature is
//! `result<_, string>`.

use thiserror::Error;

/// Errors `run_module` can fail with.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RunnerError {
    #[error("could not derive HTTP base from WS_SERVER_URL={ws_url}")]
    InvalidWsUrl { ws_url: String },

    #[error("ws-server REST call failed: {0}")]
    Rest(String),

    #[error("module {module} package.json invalid JSON: {error}")]
    PackageJsonInvalid { module: String, error: String },

    #[error("module {module} package.json missing `main` field")]
    PackageJsonMissingMain { module: String },

    #[error(transparent)]
    Wasm(#[from] wasmtime::Error),

    #[error("module run() returned err: {0}")]
    Guest(String),
}

impl<E: std::fmt::Debug> From<et_rest_client::Error<E>> for RunnerError {
    fn from(err: et_rest_client::Error<E>) -> Self {
        Self::Rest(format!("{err}"))
    }
}

/// Maps any `Display` error (the progenitor `ByteStream` chunk error in
/// practice) into `RunnerError::Rest` with a `"context: source"` prefix.
pub trait RestErrExt<T> {
    fn rest_context(self, context: &str) -> Result<T, RunnerError>;
}

impl<T, E: std::fmt::Display> RestErrExt<T> for Result<T, E> {
    fn rest_context(self, context: &str) -> Result<T, RunnerError> {
        self.map_err(|err| RunnerError::Rest(format!("{context}: {err}")))
    }
}

/// Maps any `Display` error into `RunnerError::PackageJsonInvalid` carrying
/// the module name alongside the source — used by the `fetch_main_field`
/// JSON-decode site, the only place that needs both fields.
pub trait PackageJsonErrExt<T> {
    fn package_json_err(self, module: &str) -> Result<T, RunnerError>;
}

impl<T, E: std::fmt::Display> PackageJsonErrExt<T> for Result<T, E> {
    fn package_json_err(self, module: &str) -> Result<T, RunnerError> {
        self.map_err(|err| RunnerError::PackageJsonInvalid {
            module: module.to_string(),
            error: err.to_string(),
        })
    }
}

/// Maps `Result<T, String>` (the guest's `entry.run` failure shape) into
/// `RunnerError::Guest`. Defined here so the call site in `run_module_inner`
/// stays `map_err`-free.
pub trait GuestErrExt<T> {
    fn guest_err(self) -> Result<T, RunnerError>;
}

impl<T> GuestErrExt<T> for Result<T, String> {
    fn guest_err(self) -> Result<T, RunnerError> {
        self.map_err(RunnerError::Guest)
    }
}
