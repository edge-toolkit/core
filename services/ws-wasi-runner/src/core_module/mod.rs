//! Run a Wasm 1.0 core module on NASA's `SpaceWasm` interpreter, hosting the `et` imports `et-core-guest` defines.
//!
//! A component runs on wasmtime. A core module cannot import the `et:ws-wasi` world, so it runs here instead, its
//! imports carried out on whatever [`CoreHost`] the caller hands [`run()`]. An import that waits on the hub pauses
//! the interpreter, and [`run()`] awaits it and resumes the guest; otherwise the interpreter runs in slices of
//! instructions, yielding between them, so a guest computing for a long time never keeps its executor from everything
//! else.
//!
//! `no_std` with `alloc`, so that a board can host a core module as a host does.

mod allocator;
mod error;
mod header;
mod host;
mod imports;
mod run;
mod stream;

#[cfg(feature = "board")]
pub(crate) use self::allocator::LinearMemory;
pub use self::error::CoreError;
pub use self::header::is_core_module;
pub use self::host::CoreHost;
pub use self::run::run;
#[cfg(feature = "board")]
pub(crate) use self::run::run_within;
