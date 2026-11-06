//! Run a component compiled ahead of time for a board, with the `runner` world and WASI hosted by the board.

use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Engine, Store};

use super::host::BoardHost;
use super::memory::MemoryBudget;
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
    /// The core module did not run to completion, its own `run` returning a failure among the reasons.
    #[cfg(feature = "core-modules")]
    #[error("{0}")]
    CoreModule(crate::core_module::CoreError),
}

impl From<wasmtime::Error> for BoardError {
    fn from(err: wasmtime::Error) -> Self {
        Self::Wasmtime(err)
    }
}

#[cfg(feature = "core-modules")]
impl From<crate::core_module::CoreError> for BoardError {
    fn from(err: crate::core_module::CoreError) -> Self {
        Self::CoreModule(err)
    }
}

/// Run `component` on `engine` until its `entry.run` returns, hosting it on what the board hands it.
///
/// `engine` is [`crate::engine::board_engine`]'s, and `component` what `engine::precompile` compiled for it: a firmware
/// builds its guest in, compiled by its build script. The guest runs on `platform`, reaches the hub and its storage
/// through `transport`, and every guest a board runs is handed the same `memory` for its memories and tables.
///
/// # Errors
///
/// When `component` was not compiled for `engine`, the guest imports something a board does not host, starts with more
/// memory than `memory` has left, traps, or its `entry.run` returns an error.
pub async fn run<P, T>(
    engine: &Engine,
    component: &[u8],
    platform: P,
    transport: T,
    memory: MemoryBudget,
) -> Result<(), BoardError>
where
    P: Platform + 'static,
    T: Transport + 'static,
{
    #[expect(
        unsafe_code,
        reason = "a precompiled component is native code to wasmtime, so loading one is unsafe by nature"
    )]
    // SAFETY: the bytes are what `precompile` produced for this engine's settings, built into the firmware by its build
    // script rather than received from anywhere at run time.
    let component = unsafe { Component::deserialize(engine, component) }?;
    let mut linker = Linker::new(engine);
    wasi::add_to_linker(&mut linker)?;
    bindings::Runner::add_to_linker::<BoardHost<P, T>, HasSelf<BoardHost<P, T>>>(&mut linker, |host| host)?;
    let mut store = Store::new(engine, BoardHost::new(platform, transport, memory));
    store.limiter(|host| host.allowance());
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
