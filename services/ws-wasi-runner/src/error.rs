//! Error type for `run_module`.

use thiserror::Error;

use crate::bindings::exports::et::ws_wasi::entry::EntryError;

/// Errors `run_module` can fail with.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RunnerError {
    #[error(transparent)]
    Common(#[from] et_ws_runner_common::BootstrapError),

    #[error(transparent)]
    Rest(#[from] et_rest_client::Error<()>),

    #[error(transparent)]
    Wasm(#[from] wasmtime::Error),

    #[error("module run() returned err: {0:?}")]
    Guest(#[from] EntryError),
}
