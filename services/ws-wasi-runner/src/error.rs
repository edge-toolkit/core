//! Error type for `run_module`, plus the extension trait that carries the
//! only `map_err` callsite for the runner-level error variants.
//!
//! `et_rest_client::Error`, `reqwest::Error`, `wasmtime::Error`, and the
//! WIT-defined `EntryError` already carry enough context to be useful on
//! their own, so they're forwarded transparently. Guest `run()` failures
//! arrive as `EntryError` (the variant declared on `interface entry` in
//! `generated/specs/wit/world.wit`); `?` then converts via
//! `Guest(#[from] EntryError)`.

use thiserror::Error;

use crate::bindings::exports::et::ws_wasi::entry::EntryError;

/// Errors `run_module` can fail with.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RunnerError {
    #[error("could not derive HTTP base from WS_SERVER_URL={ws_url}")]
    InvalidWsUrl { ws_url: String },

    #[error(transparent)]
    Rest(#[from] et_rest_client::Error<()>),

    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),

    #[error("module {module} package.json invalid JSON: {error}")]
    PackageJsonInvalid { module: String, error: String },

    #[error("module {module} package.json missing `main` field")]
    PackageJsonMissingMain { module: String },

    #[error(transparent)]
    Wasm(#[from] wasmtime::Error),

    #[error("module run() returned err: {0:?}")]
    Guest(#[from] EntryError),
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
