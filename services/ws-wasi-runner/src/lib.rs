//! Native runner that executes ws-modules compiled to WASI Preview 2 components.
//!
//! Counterpart to `et-ws-worker`: that crate runs browser-targeted WASM modules
//! inside an embedded V8 (rustyscript). This crate runs WASI components inside
//! `wasmtime`, with host imports for ws-server interaction (websocket, storage,
//! logging, sleep) and a wgpu-backed trimmed `wasi:webgpu/webgpu` interface
//! (subset of WebAssembly/wasi-gfx) for real GPU compute access.
//!
//! See `wit/world.wit` for the host/guest contract.

// `RunnerError` ends up large because `et_rest_client::Error<()>` carries an
// inline `reqwest::Response` (≈136 B). Boxing the variant would shave the
// parent enum but cost a `From` impl per variant; not worth it for an
// internal crate.
#![expect(
    clippy::result_large_err,
    reason = "et_rest_client::Error<()> dominates the footprint; boxing would force per-variant From impls"
)]

use futures_util::StreamExt as _;
use tracing::Instrument as _;
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine, Store};

pub mod bindings;
pub mod error;
pub mod host;

use self::error::PackageJsonErrExt as _;
pub use self::error::RunnerError;
pub use self::host::HostState;

/// Convert a WebSocket URL to its HTTP base.
pub fn derive_http_base(ws_url: &str) -> Result<String, RunnerError> {
    let (scheme, rest) = if let Some(suffix) = ws_url.strip_prefix("wss://") {
        ("https", suffix)
    } else if let Some(suffix) = ws_url.strip_prefix("ws://") {
        ("http", suffix)
    } else {
        return Err(RunnerError::InvalidWsUrl {
            ws_url: ws_url.to_string(),
        });
    };
    let host_port = rest.strip_suffix("/ws").unwrap_or(rest);
    Ok(format!("{scheme}://{host_port}"))
}

/// Drain a progenitor `ByteStream` into a `Vec<u8>`.
async fn collect_byte_stream(mut stream: et_rest_client::ByteStream) -> Result<Vec<u8>, RunnerError> {
    let mut buf = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// Read the module's `package.json` from the ws-server and extract its `main`
/// field, which names the WASI component binary.
#[expect(
    clippy::single_call_fn,
    reason = "named helper; kept separate so the package.json fetch span scopes cleanly"
)]
async fn fetch_main_field(client: &et_rest_client::Client, module_name: &str) -> Result<String, RunnerError> {
    let response = client
        .get_module_file(module_name, "package.json")
        .instrument(tracing::info_span!("fetch_package_json", module = module_name))
        .await?;
    let bytes = collect_byte_stream(response.into_inner()).await?;
    let pkg: serde_json::Value = serde_json::from_slice(&bytes).package_json_err(module_name)?;
    pkg.get("main")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .ok_or_else(|| RunnerError::PackageJsonMissingMain {
            module: module_name.to_string(),
        })
}

/// Download, link, and run the WASI component for `module_name`.
///
/// Returns when the guest's exported `entry.run` finishes (either by
/// returning `ok` or trapping). Guest `err` returns are surfaced as
/// `RunnerError::Guest`.
///
/// The whole call is wrapped in a `run_module` span — every outgoing
/// request inherits its trace context, and ws-server's request span ends
/// up as a child of it.
pub async fn run_module(module_name: &str, ws_url: &str) -> Result<(), RunnerError> {
    let span = tracing::info_span!("run_module", module = module_name);
    run_module_inner(module_name, ws_url).instrument(span).await
}

#[expect(
    clippy::single_call_fn,
    reason = "span-instrumented body of run_module; the split is mandatory to scope the tracing span"
)]
async fn run_module_inner(module_name: &str, ws_url: &str) -> Result<(), RunnerError> {
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
    }
    let engine = Engine::new(&config)?;

    let component = Component::from_binary(&engine, &wasm_bytes)?;

    let mut linker: Linker<HostState> = Linker::new(&engine);
    wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
    bindings::Runner::add_to_linker::<HostState, HasSelf<HostState>>(&mut linker, |state| state)?;
    wasmtime_wasi_nn::wit::add_to_linker(&mut linker, host::wasi_nn::view)?;

    let host_state = HostState::new(&http_base, ws_url.to_string());
    let mut store = Store::new(&engine, host_state);

    let module = bindings::Runner::instantiate_async(&mut store, &component, &linker).await?;

    module.et_ws_wasi_entry().call_run(&mut store).await??;
    Ok(())
}
