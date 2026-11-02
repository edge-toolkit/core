//! Run a component compiled ahead of time for a board, with the `runner` world and WASI hosted by the board.

use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Engine, Store};

use super::host::BoardHost;
use super::platform::{Platform, Transport};
use super::wasi;
use crate::bindings;
use crate::bindings::exports::et::ws_wasi::entry::EntryError;

/// Why a board's guest did not run to completion.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BoardError {
    /// The component could not be loaded, linked or instantiated, or trapped while it ran.
    #[error("{0}")]
    Wasmtime(wasmtime::Error),
    /// The guest's `entry.run` returned an error.
    #[error("the guest failed: {0:?}")]
    Guest(EntryError),
}

impl From<wasmtime::Error> for BoardError {
    fn from(err: wasmtime::Error) -> Self {
        Self::Wasmtime(err)
    }
}

/// Run `component` on `engine` until its `entry.run` returns, hosting it on `platform` and `transport`.
///
/// `engine` is [`crate::engine::board_engine`]'s, and `component` what [`crate::engine::precompile`] compiled for
/// it: a firmware builds its guest in, compiled by its build script.
///
/// # Errors
///
/// When `component` was not compiled for `engine`, the guest imports something a board does not host, it traps,
/// or its `entry.run` returns an error.
pub async fn run<P, T>(engine: &Engine, component: &[u8], platform: P, transport: T) -> Result<(), BoardError>
where
    P: Platform + 'static,
    T: Transport + 'static,
{
    #[expect(
        unsafe_code,
        reason = "a precompiled component is native code to wasmtime, so loading one is unsafe by nature"
    )]
    // SAFETY: the bytes are what `precompile` produced for this engine's settings, built into the firmware by its
    // build script rather than received from anywhere at run time.
    let component = unsafe { Component::deserialize(engine, component) }?;
    let mut linker = Linker::new(engine);
    wasi::add_to_linker(&mut linker)?;
    bindings::Runner::add_to_linker::<BoardHost<P, T>, HasSelf<BoardHost<P, T>>>(&mut linker, |host| host)?;
    let mut store = Store::new(engine, BoardHost::new(platform, transport));
    let runner = bindings::Runner::instantiate_async(&mut store, &component, &linker).await?;
    // `entry.run` is an async export, which only the concurrent API can call.
    match store
        .run_concurrent(async |accessor| runner.et_ws_wasi_entry().call_run(accessor).await)
        .await??
    {
        Ok(()) => Ok(()),
        Err(error) => Err(BoardError::Guest(error)),
    }
}
