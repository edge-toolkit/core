//! Native runner that executes ws-modules compiled to WASI Preview 2 components.
//!
//! Counterpart to `et-ws-worker`: that crate runs browser-targeted WASM modules
//! inside an embedded V8 (rustyscript). This crate runs WASI components inside
//! `wasmtime`, with host imports for ws-server interaction (websocket, storage,
//! logging, sleep) and a wgpu-backed trimmed `wasi:webgpu/webgpu` interface
//! (subset of WebAssembly/wasi-gfx) for real GPU compute access.
//!
//! See `wit/world.wit` for the host/guest contract.

use et_ws_runner_common::{collect_byte_stream, derive_http_base, fetch_main_field};
use tracing::Instrument as _;
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine, Store};

pub mod bindings;
pub mod config;
pub mod error;
pub mod host;

pub use self::error::RunnerError;
pub use self::host::HostState;

/// Download, link, and run the WASI component for `module_name`.
///
/// Returns when the guest's exported `entry.run` finishes (either by
/// returning `ok` or trapping). Guest `err` returns are surfaced as
/// `RunnerError::Guest`.
///
/// The whole call is wrapped in a `run_module` span -- every outgoing
/// request inherits its trace context, and ws-server's request span ends
/// up as a child of it.
pub async fn run_module(
    module_name: &str,
    ws_url: &str,
    connect_ack_timeout: Option<std::time::Duration>,
    coverage: bool,
) -> Result<(), RunnerError> {
    let span = tracing::info_span!("run_module", module = module_name);
    run_module_inner(module_name, ws_url, connect_ack_timeout, coverage)
        .instrument(span)
        .await
}

#[expect(
    clippy::single_call_fn,
    reason = "span-instrumented body of run_module; the split is mandatory to scope the tracing span"
)]
async fn run_module_inner(
    module_name: &str,
    ws_url: &str,
    connect_ack_timeout: Option<std::time::Duration>,
    coverage: bool,
) -> Result<(), RunnerError> {
    let http_base = derive_http_base(ws_url)?;

    let rest = et_rest_client::Client::new(&http_base);
    let main = fetch_main_field(&rest, module_name).await?;
    tracing::info!(module = module_name, %main, "fetching WASI component");
    let response = rest
        .get_module_file(module_name, &main)
        .instrument(tracing::info_span!("fetch_component", module = module_name, file = %main))
        .await?;
    let wasm_bytes = collect_byte_stream(response.into_inner()).await?;

    let mut config = Config::new();
    #[expect(
        unused_results,
        reason = "wasmtime::Config::wasm_component_model returns &mut Self for builder chaining; mutation is the intent"
    )]
    {
        config.wasm_component_model(true);
        // wasi:webgpu@0.3.0-rc.2 declares `request-adapter`, `request-device` and `map-async` as
        // `async func`, which is component-model-async (WASI Preview 3) ABI. Without this the guest's
        // webgpu imports fail to instantiate.
        config.wasm_component_model_async(true);
    }
    let engine = Engine::new(&config)?;

    let component = Component::from_binary(&engine, &wasm_bytes)?;

    let mut linker: Linker<HostState> = Linker::new(&engine);
    wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
    bindings::Runner::add_to_linker::<HostState, HasSelf<HostState>>(&mut linker, |state| state)?;
    wasmtime_wasi_nn::wit::add_to_linker(&mut linker, host::wasi_nn::view)?;
    wasi_webgpu_wasmtime::add_to_linker(&mut linker)?;

    let host_state = HostState::new(&http_base, ws_url.to_string(), connect_ack_timeout, coverage);
    let mut store = Store::new(&engine, host_state);

    let module = bindings::Runner::instantiate_async(&mut store, &component, &linker).await?;

    // `entry.run` is an async export, so it is driven through the concurrent API rather than called
    // directly against the store: the guest may await a host import (wasi-webgpu's `request-adapter`,
    // `request-device`, `map-async`) mid-call, and only an `Accessor` can hand the store back to the
    // host while the guest task is suspended.
    store
        .run_concurrent(async |accessor| module.et_ws_wasi_entry().call_run(accessor).await)
        .await???;
    Ok(())
}
