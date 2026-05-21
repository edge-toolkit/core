//! Native runner that executes ws-modules compiled to WASI Preview 2 components.
//!
//! Counterpart to `et-ws-worker`: that crate runs browser-targeted WASM modules
//! inside an embedded V8 (rustyscript). This crate runs WASI components inside
//! `wasmtime`, with host imports for ws-server interaction (websocket, storage,
//! logging, sleep) and a wgpu-backed trimmed `wasi:webgpu/webgpu` interface
//! (subset of WebAssembly/wasi-gfx) for real GPU compute access.
//!
//! See `wit/world.wit` for the host/guest contract.

use opentelemetry_http::HeaderInjector;
use thiserror::Error;
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine, Store};

/// Errors `run_module` can fail with. Both `reqwest::Error` and
/// `wasmtime::Error` already carry enough context (the failing URL,
/// the wasmtime error chain) to be useful on their own, so they're
/// forwarded transparently.
#[derive(Debug, Error)]
pub enum RunnerError {
    #[error("could not derive HTTP base from WS_SERVER_URL={ws_url}")]
    InvalidWsUrl { ws_url: String },

    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error("module {module} package.json missing `main` field")]
    PackageJsonMissingMain { module: String },

    #[error(transparent)]
    Wasm(#[from] wasmtime::Error),

    #[error("module run() returned err: {0}")]
    Guest(String),
}

pub mod bindings;

pub mod host;

pub use host::HostState;

/// Inject the W3C `traceparent` (and any `tracestate`) for the current span
/// into `req`. Downstream HTTP servers running `tracing-actix-web`'s
/// `TracingLogger` (or any propagator-aware middleware) parent their
/// request span on the value, which is how a single trace id covers both
/// processes.
fn inject_traceparent(req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    let mut headers = reqwest::header::HeaderMap::new();
    let cx = tracing::Span::current().context();
    opentelemetry::global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&cx, &mut HeaderInjector(&mut headers));
    });
    req.headers(headers)
}

/// Convert a `ws://host[:port]/ws` URL to its `http://host[:port]` HTTP base
/// (or `wss://` → `https://`). Returns `None` if `ws_url` is not a websocket
/// URL.
pub fn derive_http_base(ws_url: &str) -> Option<String> {
    let (scheme, rest) = if let Some(r) = ws_url.strip_prefix("wss://") {
        ("https", r)
    } else if let Some(r) = ws_url.strip_prefix("ws://") {
        ("http", r)
    } else {
        return None;
    };
    let host_port = rest.strip_suffix("/ws").unwrap_or(rest);
    Some(format!("{scheme}://{host_port}"))
}

/// Where to find the .wasm component for a given module.
///
/// Resolved against `package.json`'s `main` field as served by the ws-server.
async fn resolve_component_url(http_base: &str, module_name: &str) -> Result<String, RunnerError> {
    let pkg_url = format!("{http_base}/modules/{module_name}/package.json");
    let pkg: serde_json::Value = inject_traceparent(reqwest::Client::new().get(&pkg_url))
        .send()
        .instrument(tracing::info_span!("fetch_package_json", url = %pkg_url))
        .await?
        .error_for_status()?
        .json()
        .await?;
    let main = pkg
        .get("main")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RunnerError::PackageJsonMissingMain {
            module: module_name.to_string(),
        })?;
    Ok(format!("{http_base}/modules/{module_name}/{main}"))
}

/// Download, link, and run the WASI component for `module_name`. Returns when
/// the guest's exported `entry.run` finishes (either by returning `ok` or
/// trapping). Guest `err` returns are surfaced as `RunnerError::Guest`.
///
/// The whole call is wrapped in a `run_module` span — every outgoing
/// request inherits its trace context, and ws-server's request span ends
/// up as a child of it.
pub async fn run_module(module_name: &str, ws_url: &str) -> Result<(), RunnerError> {
    let span = tracing::info_span!("run_module", module = module_name);
    run_module_inner(module_name, ws_url).instrument(span).await
}

async fn run_module_inner(module_name: &str, ws_url: &str) -> Result<(), RunnerError> {
    let http_base = derive_http_base(ws_url).ok_or_else(|| RunnerError::InvalidWsUrl {
        ws_url: ws_url.to_string(),
    })?;

    let wasm_url = resolve_component_url(&http_base, module_name).await?;
    tracing::info!(%wasm_url, "fetching WASI component");
    let wasm_bytes = inject_traceparent(reqwest::Client::new().get(&wasm_url))
        .send()
        .instrument(tracing::info_span!("fetch_component", url = %wasm_url))
        .await?
        .error_for_status()?
        .bytes()
        .await?;

    let mut config = Config::new();
    config.wasm_component_model(true);
    let engine = Engine::new(&config)?;

    let component = Component::from_binary(&engine, &wasm_bytes)?;

    let mut linker: Linker<HostState> = Linker::new(&engine);
    wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
    bindings::Runner::add_to_linker::<HostState, HasSelf<HostState>>(&mut linker, |s| s)?;
    wasmtime_wasi_nn::wit::add_to_linker(&mut linker, host::wasi_nn::view)?;

    let host_state = HostState::new(http_base, ws_url.to_string()).await;
    let mut store = Store::new(&engine, host_state);

    let module = bindings::Runner::instantiate_async(&mut store, &component, &linker).await?;

    let guest_result = module.et_ws_wasi_entry().call_run(&mut store).await?;

    guest_result.map_err(RunnerError::Guest)
}
