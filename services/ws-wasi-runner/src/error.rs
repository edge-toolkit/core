//! Error type for `run_module`.
//!
//! `et_rest_client::Error`, `reqwest::Error`, `wasmtime::Error`, the
//! WIT-defined `EntryError`, and `serde_path_to_error::Error` already carry
//! enough context to be useful on their own, so they're forwarded
//! transparently via `#[from]`. Guest `run()` failures arrive as
//! `EntryError` (the variant declared on `interface entry` in
//! `generated/specs/wit/world.wit`).

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

    #[error(transparent)]
    PackageJsonInvalid(#[from] serde_path_to_error::Error<serde_json::Error>),

    #[error("module {module} package.json missing `main` field")]
    PackageJsonMissingMain { module: String },

    #[error(transparent)]
    Wasm(#[from] wasmtime::Error),

    #[error("module run() returned err: {0:?}")]
    Guest(#[from] EntryError),
}
