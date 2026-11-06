//! Run a board's guest, of whichever kind the build built in, until it finishes, as a supervisor runs a runner process.

use core::fmt::Display;

use super::memory::MemoryBudget;
use super::platform::{Console, Platform, Transport};
use super::run::{BoardError, run};
#[cfg(feature = "core-modules")]
use super::run_core::run_core_module;
#[cfg(feature = "core-modules")]
use crate::core_module::{CoreError, is_core_module};
use crate::engine::board_engine;

/// How long, in nanoseconds of the board's clock, a guest whose run failed waits before it is run again.
pub const RERUN_DELAY: u64 = 5_000_000_000;

/// Run the guest `name` on `platform` until it finishes, writing how each run ended to `platform`'s stderr.
///
/// `guest` is the guest as a firmware builds it in: a component compiled for [`board_engine`], which runs on that,
/// or with the `core-modules` feature a core module as its package holds it, which runs on the board's interpreter.
/// It reaches its hub through a transport from `transport`, and keeps its memories in `memory`. A run the guest
/// itself fails, as it does when its hub drops the connection under it, is run again on a fresh transport after
/// [`RERUN_DELAY`], as a supervisor restarts a runner process; one that cannot load the guest, or that the guest traps
/// in, is not, since it would fail the same way every time. How each run ended is written rather than returned, since
/// nothing waits on a runner: the hub carries on without it.
#[cfg_attr(
    feature = "core-modules",
    expect(
        clippy::future_not_send,
        reason = "SpaceWasm's engine is neither Send nor Sync, and a board runs each guest's task on its one core"
    )
)]
pub async fn run_guest<P, T, N>(name: &str, guest: &[u8], platform: P, mut transport: N, memory: MemoryBudget)
where
    P: Platform + Clone + 'static,
    T: Transport + 'static,
    N: FnMut() -> T,
{
    #[cfg(feature = "core-modules")]
    if is_core_module(guest) {
        let run_once = || run_core_module(guest, platform.clone(), transport(), memory.clone());
        return rerunning(&platform, name, run_once).await;
    }
    let engine = match board_engine() {
        Ok(engine) => engine,
        Err(error) => return report(&platform, name, BoardError::Wasmtime(error)),
    };
    let run_once = || run(&engine, guest, platform.clone(), transport(), memory.clone());
    rerunning(&platform, name, run_once).await;
}

/// Run the guest `name` through `run_once` until a run does not fail in the guest itself, reporting how each ended.
#[cfg_attr(
    not(feature = "core-modules"),
    expect(
        clippy::single_call_fn,
        reason = "the supervision of run_guest, which runs a core module through it too when the crate has them"
    )
)]
async fn rerunning<P, R, F>(platform: &P, name: &str, mut run_once: R)
where
    P: Platform,
    R: FnMut() -> F,
    F: Future<Output = Result<(), BoardError>>,
{
    loop {
        match run_once().await {
            Ok(()) => return report(platform, name, "finished"),
            Err(error) if failed_in_guest(&error) => {
                report(platform, name, format_args!("{error}; running it again"));
                platform.sleep_until(platform.now().saturating_add(RERUN_DELAY)).await;
            }
            Err(error) => return report(platform, name, error),
        }
    }
}

/// Whether `error` is the guest's own failure, which a run on a fresh transport need not repeat.
#[expect(
    clippy::single_call_fn,
    reason = "the rerun decision of rerunning, named for the one question it answers"
)]
const fn failed_in_guest(error: &BoardError) -> bool {
    match error {
        BoardError::Guest(_) => true,
        #[cfg(feature = "core-modules")]
        BoardError::CoreModule(error) => matches!(error, CoreError::Guest(_)),
        BoardError::Wasmtime(_) => false,
    }
}

/// Write how a run of the guest `name` ended to `platform`'s stderr.
fn report<P, E>(platform: &P, name: &str, ended: E)
where
    P: Platform,
    E: Display,
{
    platform.write(Console::Stderr, alloc::format!("runner {name}: {ended}\n").as_bytes());
}
