//! Error type for `run_module`.

use thiserror::Error;

use crate::bindings::exports::et::ws_wasi::entry::EntryError;

/// Errors `run_module` can fail with.
///
/// The two foreign variants are boxed. Both source types are ~136 bytes, which pushed every
/// `Result<_, RunnerError>` past `clippy::result_large_err`'s threshold and made the whole enum expensive
/// to move on the happy path. The hand-written `From` impls below keep `?` converting from the unboxed
/// source, so boxing stays invisible at the call sites.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RunnerError {
    #[error(transparent)]
    Common(Box<et_ws_runner_common::BootstrapError>),

    #[error(transparent)]
    Rest(Box<et_rest_client::Error<()>>),

    #[error(transparent)]
    Wasm(#[from] wasmtime::Error),

    #[error("module run() returned err: {0:?}")]
    Guest(#[from] EntryError),
}

impl From<et_ws_runner_common::BootstrapError> for RunnerError {
    fn from(err: et_ws_runner_common::BootstrapError) -> Self {
        Self::Common(Box::new(err))
    }
}

impl From<et_rest_client::Error<()>> for RunnerError {
    fn from(err: et_rest_client::Error<()>) -> Self {
        Self::Rest(Box::new(err))
    }
}
