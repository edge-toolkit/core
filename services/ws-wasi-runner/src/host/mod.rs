//! Host-side implementation of the `et:ws-wasi` WIT world.
//!
//! `HostState` is the per-store object held by `wasmtime::Store<HostState>`. It
//! owns the WASI Preview 2 context (for stdio/env/random/etc.), an HTTP client
//! used by the storage interface, the ws connection state, and the wgpu device
//! used by the gfx interface.

use std::sync::Arc;

use tokio::sync::Mutex;
use wasmtime::component::ResourceTable;
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

mod error;
mod log;
pub mod wasi_keyvalue;
pub mod wasi_nn;
pub mod wasi_webgpu;
mod ws;

pub use et_wasi::error::WitErrExt;

pub use self::error::{KvErrExt, RequestDeviceErrExt};
pub use self::ws::WsBackend;

pub struct HostState {
    pub wasi_ctx: WasiCtx,
    pub resource_table: ResourceTable,

    /// HTTP base of the ws-server (e.g. `http://localhost:8080`).
    pub http_base: String,
    /// WebSocket URL of the ws-server (e.g. `ws://localhost:8080/ws`).
    pub ws_url: String,

    pub http: reqwest::Client,
    pub ws: Arc<Mutex<Option<WsBackend>>>,
    /// wasi-nn context. Constructed once at startup so model loads + compute
    /// reuse the same `ort` session pool across calls.
    pub wasi_nn_ctx: wasmtime_wasi_nn::wit::WasiNnCtx,
}

impl HostState {
    pub async fn new(http_base: String, ws_url: String) -> Self {
        let wasi_ctx = WasiCtxBuilder::new().inherit_stdio().inherit_env().build();

        Self {
            wasi_ctx,
            resource_table: ResourceTable::new(),
            http_base,
            ws_url,
            http: reqwest::Client::new(),
            ws: Arc::new(Mutex::new(None)),
            wasi_nn_ctx: wasi_nn::new_ctx(),
        }
    }
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi_ctx,
            table: &mut self.resource_table,
        }
    }
}
