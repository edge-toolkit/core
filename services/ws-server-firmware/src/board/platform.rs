//! What every board hands its guests, whatever hub they reach: its clock and its log, and the heap it starts with.

use defmt::{Debug2Format, info, warn};
use embassy_executor::{SpawnError, SpawnToken, Spawner};
use embassy_time::{Instant, Timer};
use et_ws_wasi_runner::bindings::wasi::logging::logging;
use et_ws_wasi_runner::board::{Console, Platform};

/// The heap the hub's registry, routing and frame buffers allocate from, on a board whose heap is its SRAM.
#[cfg(not(feature = "pico-plus2w"))]
pub(super) const HEAP_SIZE: usize = 160 * 1024;
/// Nanoseconds in a microsecond: a guest's clock counts the one, the board's the other.
const NANOS_PER_MICRO: u64 = 1000;

/// Start the task running the guest `name`, or log why it could not be.
#[expect(
    clippy::single_call_fn,
    reason = "shared by the board's two roles, each of which a build has only one of"
)]
pub(super) fn spawn_runner<S>(spawner: Spawner, task: Result<SpawnToken<S>, SpawnError>, name: &str) {
    match task {
        Ok(task) => spawner.spawn(task),
        Err(error) => warn!("Failed to start runner {=str}: {}", name, Debug2Format(&error)),
    }
}

/// The board's clock and its defmt log, which its guests' clock and log are.
#[derive(Clone, Copy)]
pub(super) struct BoardPlatform;

impl Platform for BoardPlatform {
    /// The board's uptime, in the nanoseconds a guest's clock counts.
    fn now(&self) -> u64 {
        Instant::now().as_micros().saturating_mul(NANOS_PER_MICRO)
    }

    /// Wait on the board's timer until its uptime reaches `deadline`.
    async fn sleep_until(&self, deadline: u64) {
        Timer::at(Instant::from_micros(deadline.div_euclid(NANOS_PER_MICRO))).await;
    }

    /// Log the guest's entry at the defmt level nearest its own.
    #[expect(
        clippy::missing_asserts_for_indexing,
        reason = "the indexing is inside defmt's error! expansion, which an attribute on the call does not reach"
    )]
    fn log(&self, level: logging::Level, context: &str, message: &str) {
        match level {
            logging::Level::Critical | logging::Level::Error => defmt::error!("{}: {}", context, message),
            logging::Level::Warn => warn!("{}: {}", context, message),
            logging::Level::Info => info!("{}: {}", context, message),
            logging::Level::Debug | logging::Level::Trace => defmt::debug!("{}: {}", context, message),
        }
    }

    /// Log what the guest wrote to either standard stream, the board's one console being its defmt log.
    fn write(&self, _console: Console, bytes: &[u8]) {
        info!(
            "{=str}",
            core::str::from_utf8(bytes).unwrap_or("(output that is not UTF-8)")
        );
    }
}
