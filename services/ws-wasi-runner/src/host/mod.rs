//! Host-side implementation of the `et:ws-wasi` WIT world.
//!
//! `HostState` is the per-store object held by `wasmtime::Store<HostState>`. It
//! owns the WASI Preview 2 context (for stdio/env/random/etc.), an HTTP client
//! used by the storage interface, the ws connection state, and the wgpu device
//! used by the gfx interface.

#[cfg(feature = "core-modules")]
mod core_module;
mod error;
mod log;
mod run;
#[cfg(feature = "lidar")]
pub mod serial;
mod state;
pub mod wasi_keyvalue;
#[cfg(feature = "nn")]
pub mod wasi_nn;
pub mod ws;

#[cfg(feature = "core-modules")]
pub use self::core_module::run_core_module;
pub use self::run::run_module;
pub use self::state::HostState;
pub use self::ws::WsBackend;
