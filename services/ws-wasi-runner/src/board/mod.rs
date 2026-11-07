//! The runner for a board with no operating system.
//!
//! It runs a component compiled ahead of time for the board's engine, with the `runner` world and the WASI Preview 2
//! its guest imports hosted on what the board provides, and with the `core-modules` feature a Wasm 1.0 core module on
//! `SpaceWasm`, with the `et` imports hosted on the same.

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
#[cfg(feature = "core-modules")]
mod run_core;
mod run_guest;
#[cfg(test)]
mod test_host;
mod wasi;

pub use self::buckets::{Absent, Buckets};
pub use self::host::BoardHost;
pub use self::memory::MemoryBudget;
pub use self::platform::{Console, Platform, Transport};
pub use self::run::{BoardError, run};
#[cfg(feature = "core-modules")]
pub use self::run_core::run_core_module;
pub use self::run_guest::{RERUN_DELAY, run_guest};
