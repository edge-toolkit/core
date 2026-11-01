//! The engine a board runs a component with, and compiling a component for it ahead of time.
//!
//! A board has no compiler and no virtual memory, so its engine runs Pulley bytecode a host compiled for it, keeps
//! each linear memory to what it holds, and checks bounds and traps explicitly rather than through signals. The host
//! has to compile with the same settings the board loads with, so both come from [`board_config`].

#[cfg(all(feature = "precompile", not(feature = "std")))]
use alloc::vec::Vec;

use wasmtime::{Config, Engine};

/// The Pulley target a board with this build's pointer width runs.
pub const NATIVE_PULLEY: &str = if cfg!(target_pointer_width = "64") {
    "pulley64"
} else {
    "pulley32"
};
/// The stack each guest call runs on, the most a board gives one.
///
/// The interpreter and the host functions a guest calls run on it too, and unoptimised their frames are several
/// times larger, so a debug build gives it four times the room.
const ASYNC_STACK_SIZE: usize = if cfg!(debug_assertions) {
    // 64 KiB overflowed on Windows, on `override (msvc)` at commit
    // https://github.com/edge-toolkit/core/commit/0a3156acdc5f4405cb318451a58ace3c53922d5c
    // (`https://github.com/edge-toolkit/core/actions/runs/41308264184/job/124853064168`):
    //
    //     thread 'data1_round_trips_a_value_through_the_boards_memory' (876) has overflowed its stack
    256
} else {
    64
} * 1024;
/// The guest's share of [`ASYNC_STACK_SIZE`]; the rest is the host functions' it calls.
const MAX_WASM_STACK: usize = 32 * 1024;

/// The settings a board's engine runs with, compiling for `target`.
///
/// # Errors
///
/// When `target` names no target this build of wasmtime knows.
pub fn board_config(target: &str) -> wasmtime::Result<Config> {
    let mut config = Config::new();
    #[expect(
        unused_results,
        reason = "each setter returns &mut Config for chaining; the mutation is the intent"
    )]
    {
        config.target(target)?;
        config.wasm_component_model_async(true);
        config.signals_based_traps(false);
        config.memory_reservation(0);
        config.memory_reservation_for_growth(0);
        config.memory_guard_size(0);
        config.async_stack_size(ASYNC_STACK_SIZE);
        config.max_wasm_stack(MAX_WASM_STACK);
    }
    Ok(config)
}

/// The engine a board runs components with, which loads only what [`precompile`] compiled for this build's target.
///
/// # Errors
///
/// When wasmtime rejects the settings, as it would one that this build cannot run.
pub fn board_engine() -> wasmtime::Result<Engine> {
    Engine::new(&board_config(NATIVE_PULLEY)?)
}

/// `component`, compiled for a board running `target`, as that board's [`board_engine`] loads it.
///
/// # Errors
///
/// When `target` is unknown, or `component` is not a valid component.
#[cfg(feature = "precompile")]
pub fn precompile(target: &str, component: &[u8]) -> wasmtime::Result<Vec<u8>> {
    Engine::new(&board_config(target)?)?.precompile_component(component)
}
