//! Host-side implementation of the `et:ws-wasi` WIT world.
//!
//! `HostState` is the per-store object held by `wasmtime::Store<HostState>`. It
//! owns the WASI Preview 2 context (for stdio/env/random/etc.), an HTTP client
//! used by the storage interface, the ws connection state, and the wgpu device
//! used by the gfx interface.

mod error;
mod log;
mod state;
pub mod wasi_keyvalue;
pub mod wasi_nn;
pub mod ws;

pub use self::error::kv_not_implemented;
pub use self::state::HostState;
pub use self::ws::WsBackend;
