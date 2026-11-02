//! The runner for a board with no operating system.
//!
//! It runs a component compiled ahead of time for the board's engine, with the `runner` world and the WASI Preview 2
//! its guest imports hosted on what the board provides.

#[cfg(feature = "coverage")]
mod cov;
mod host;
mod memory;
mod platform;
mod run;
#[cfg(test)]
mod test_host;
mod wasi;

pub use self::host::{BoardHost, Buckets};
pub use self::memory::MemoryBudget;
pub use self::platform::{Console, Platform, Transport};
pub use self::run::{BoardError, run};
