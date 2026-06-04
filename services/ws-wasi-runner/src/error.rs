//! Error type for `run_module`, plus the extension traits that carry the
//! only `map_err` callsites for the runner-level error variants.
//!
//! `et_rest_client::Error`, `reqwest::Error`, and `wasmtime::Error` already
//! carry enough context to be useful on their own, so they're forwarded
//! transparently. Guest failures arrive as `String` because the `entry.run`
//! WIT signature is `result<_, string>`.

use thiserror::Error;

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

    #[error("module run() returned err: {0}")]
    Guest(String),
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
/// `RunnerError::Guest`. Kept as a trait — and not collapsed into the call
/// site as `.map_err(RunnerError::Guest)` — because the `no-map-err`
/// ast-grep rule forbids `.map_err` outside the listed error.rs files. A
/// `From<String> for RunnerError` impl would also work, but `From<String>`
/// for an error type is far too broad: every stringly-typed conversion in
/// the crate would silently become a `RunnerError::Guest`.
pub trait GuestErrExt<T> {
    fn guest_err(self) -> Result<T, RunnerError>;
}

impl<T> GuestErrExt<T> for Result<T, String> {
    fn guest_err(self) -> Result<T, RunnerError> {
        self.map_err(RunnerError::Guest)
    }
}
