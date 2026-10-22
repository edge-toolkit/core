//! Typed modes for the recovery command subprocess fixture.

use std::time::Duration;

use clap::{Parser, ValueEnum};

/// Emit subprocess diagnostics and select when the fixture exits.
#[derive(Parser)]
#[command(name = "recovery-command-probe")]
pub struct Arguments {
    /// Completion behaviour used by the parent command test.
    #[arg(value_enum)]
    mode: Mode,
}

/// Accepted subprocess behaviours; arbitrary arguments cannot select additional actions.
#[derive(Clone, Copy, ValueEnum)]
enum Mode {
    /// Exit immediately after flushing the diagnostic streams.
    Complete,
    /// Exit after a short delay to exercise normal asynchronous collection.
    Pause,
    /// Wait for the parent's deadline while remaining finite if the parent disappears.
    Hang,
}

impl Arguments {
    /// Return the selected finite delay without sleeping so every mode can be verified directly.
    #[must_use]
    pub const fn delay(&self) -> Duration {
        match self.mode {
            Mode::Complete => Duration::ZERO,
            Mode::Pause => Duration::from_millis(25),
            Mode::Hang => Duration::from_secs(120),
        }
    }
}
