//! The runner for a board with no operating system.
//!
//! It runs a component compiled ahead of time for the board's engine, with the `runner` world and the WASI Preview 2
//! its guest imports hosted on what the board provides.

mod buckets;
#[cfg(feature = "coverage")]
mod cov;
mod host;
mod memory;
pub mod net;
#[cfg(feature = "std")]
pub mod net_tokio;
mod platform;
mod run;
#[cfg(test)]
mod test_host;
mod wasi;

pub use self::buckets::{Absent, Buckets};
pub use self::host::BoardHost;
pub use self::memory::MemoryBudget;
pub use self::platform::{Console, Platform, Transport};
pub use self::run::{BoardError, run};
