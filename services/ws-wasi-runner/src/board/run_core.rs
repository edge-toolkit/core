//! Run a Wasm 1.0 core module on a board, on `SpaceWasm`, with its `et` imports hosted by the board.

use super::host::BoardHost;
use super::memory::{CoreMemory, MemoryBudget};
use super::platform::{Platform, Transport};
use super::run::BoardError;
use crate::core_module::run_within;

/// The interpreter's operand and call stack on a board, in its four-byte words: 64 KiB, as wasmtime's stack is there.
const STACK_WORDS: usize = 0x4000;

/// Run the core module `module` until its `run` returns, hosting its `et` imports on what the board hands it.
///
/// `SpaceWasm` decodes a core module as it is, so a firmware builds one in as its package holds it, with nothing
/// compiled ahead of time. The guest runs on `platform` and reaches the hub and its storage through `transport`, as a
/// component does. Its linear memory draws on `memory` as a component's memories do; the interpreter's stack and the
/// code it decodes the module to are kept beside it, as wasmtime keeps a component's.
///
/// # Errors
///
/// When the module does not decode, validate or link, imports anything but the `et` functions, starts with more memory
/// than `memory` has left, traps, or its `run` returns a failure.
#[expect(
    clippy::future_not_send,
    reason = "SpaceWasm's engine is neither Send nor Sync, and a board runs each guest's task on its one core"
)]
pub async fn run_core_module<P, T>(
    module: &[u8],
    platform: P,
    transport: T,
    memory: MemoryBudget,
) -> Result<(), BoardError>
where
    P: Platform,
    T: Transport,
{
    let host = BoardHost::new(platform, transport, memory.clone());
    Ok(run_within(module, host, CoreMemory::new(memory), STACK_WORDS).await?)
}
