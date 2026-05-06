//! Native runner that executes ws-modules compiled to WASI Preview 2 components.
//!
//! Counterpart to `et-ws-worker`: that crate runs browser-targeted WASM modules
//! inside an embedded V8 (rustyscript). This crate runs WASI components inside
//! `wasmtime`, with host imports for ws-server interaction (websocket, storage,
//! logging, sleep) and a wgpu-backed trimmed `wasi:webgpu/webgpu` interface
//! (subset of WebAssembly/wasi-gfx) for real GPU compute access.
//!
//! See `wit/world.wit` for the host/guest contract.

use anyhow::{Context, Result};
use opentelemetry_http::HeaderInjector;
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine, Store};

pub mod bindings {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "runner",
        imports: { default: async },
        exports: { default: async },
        // Map every wasi-webgpu resource to a payload type owned by us so
        // resource_table operations work on real wgpu objects rather than
        // bindgen-generated marker structs. The types live in
        // `host::wasi_webgpu` and are wgpu-backed for the matmul subset.
        with: {
            "wasi:keyvalue/store.bucket": super::host::wasi_keyvalue::Bucket,
            "wasi:webgpu/webgpu.gpu": super::host::wasi_webgpu::Gpu,
            "wasi:webgpu/webgpu.gpu-adapter": super::host::wasi_webgpu::GpuAdapter,
            "wasi:webgpu/webgpu.gpu-adapter-info": super::host::wasi_webgpu::GpuAdapterInfo,
            "wasi:webgpu/webgpu.gpu-supported-features": super::host::wasi_webgpu::GpuSupportedFeatures,
            "wasi:webgpu/webgpu.gpu-supported-limits": super::host::wasi_webgpu::GpuSupportedLimits,
            "wasi:webgpu/webgpu.gpu-device": super::host::wasi_webgpu::GpuDevice,
            "wasi:webgpu/webgpu.gpu-queue": super::host::wasi_webgpu::GpuQueue,
            "wasi:webgpu/webgpu.gpu-buffer": super::host::wasi_webgpu::GpuBuffer,
            "wasi:webgpu/webgpu.gpu-buffer-usage": super::host::wasi_webgpu::GpuBufferUsage,
            "wasi:webgpu/webgpu.gpu-map-mode": super::host::wasi_webgpu::GpuMapMode,
            "wasi:webgpu/webgpu.gpu-shader-stage": super::host::wasi_webgpu::GpuShaderStage,
            "wasi:webgpu/webgpu.gpu-bind-group-layout": super::host::wasi_webgpu::GpuBindGroupLayout,
            "wasi:webgpu/webgpu.gpu-bind-group": super::host::wasi_webgpu::GpuBindGroup,
            "wasi:webgpu/webgpu.gpu-pipeline-layout": super::host::wasi_webgpu::GpuPipelineLayout,
            "wasi:webgpu/webgpu.gpu-shader-module": super::host::wasi_webgpu::GpuShaderModule,
            "wasi:webgpu/webgpu.gpu-compute-pipeline": super::host::wasi_webgpu::GpuComputePipeline,
            "wasi:webgpu/webgpu.gpu-command-encoder": super::host::wasi_webgpu::GpuCommandEncoder,
            "wasi:webgpu/webgpu.gpu-compute-pass-encoder": super::host::wasi_webgpu::GpuComputePassEncoder,
            "wasi:webgpu/webgpu.gpu-command-buffer": super::host::wasi_webgpu::GpuCommandBuffer,
            "wasi:webgpu/webgpu.record-option-gpu-size64": super::host::wasi_webgpu::RecordOptionGpuSize64,
            "wasi:webgpu/webgpu.record-gpu-pipeline-constant-value":
                super::host::wasi_webgpu::RecordGpuPipelineConstantValue,
        },
    });
}

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
/// Resolved against `package.json` from the ws-server. We prefer the
/// `wasi-main` field (set by Python WASI modules) and fall back to `main` so
/// that components named like `et_ws_*.wasm` Just Work.
/// Wasmtime's error type isn't `std::error::Error`, so anyhow's `Context`
/// trait can't attach extra context to it directly. Convert via `Display`.
fn into_anyhow(err: wasmtime::Error) -> anyhow::Error {
    anyhow::anyhow!("{err:#}")
}

async fn resolve_component_url(http_base: &str, module_name: &str) -> Result<String> {
    let pkg_url = format!("{http_base}/modules/{module_name}/package.json");
    let pkg: serde_json::Value = inject_traceparent(reqwest::Client::new().get(&pkg_url))
        .send()
        .instrument(tracing::info_span!("fetch_package_json", url = %pkg_url))
        .await
        .with_context(|| format!("GET {pkg_url}"))?
        .error_for_status()?
        .json()
        .await?;
    let wasi_main = pkg
        .get("wasi-main")
        .and_then(|v| v.as_str())
        .or_else(|| pkg.get("main").and_then(|v| v.as_str()))
        .with_context(|| format!("module {module_name} package.json has neither wasi-main nor main"))?;
    Ok(format!("{http_base}/modules/{module_name}/{wasi_main}"))
}

/// Download, link, and run the WASI component for `module_name`. Returns when
/// the guest's exported `entry.run` finishes (either by returning `ok` or
/// trapping). Guest `err` returns are surfaced as `anyhow::Error`.
///
/// The whole call is wrapped in a `run_module` span — every outgoing
/// request inherits its trace context, and ws-server's request span ends
/// up as a child of it.
pub async fn run_module(module_name: &str, ws_url: &str) -> Result<()> {
    let span = tracing::info_span!("run_module", module = module_name);
    run_module_inner(module_name, ws_url).instrument(span).await
}

async fn run_module_inner(module_name: &str, ws_url: &str) -> Result<()> {
    let http_base =
        derive_http_base(ws_url).with_context(|| format!("could not derive HTTP base from WS_SERVER_URL={ws_url}"))?;

    let wasm_url = resolve_component_url(&http_base, module_name).await?;
    tracing::info!(%wasm_url, "fetching WASI component");
    let wasm_bytes = inject_traceparent(reqwest::Client::new().get(&wasm_url))
        .send()
        .instrument(tracing::info_span!("fetch_component", url = %wasm_url))
        .await
        .with_context(|| format!("GET {wasm_url}"))?
        .error_for_status()?
        .bytes()
        .await?;

    let mut config = Config::new();
    config.wasm_component_model(true);
    let engine = Engine::new(&config)?;

    let component = Component::from_binary(&engine, &wasm_bytes).map_err(into_anyhow)?;

    let mut linker: Linker<HostState> = Linker::new(&engine);
    wasmtime_wasi::p2::add_to_linker_async(&mut linker).map_err(into_anyhow)?;
    bindings::Runner::add_to_linker::<HostState, HasSelf<HostState>>(&mut linker, |s| s).map_err(into_anyhow)?;
    wasmtime_wasi_nn::wit::add_to_linker(&mut linker, host::wasi_nn::view).map_err(into_anyhow)?;

    let host_state = HostState::new(http_base, ws_url.to_string()).await?;
    let mut store = Store::new(&engine, host_state);

    let module = bindings::Runner::instantiate_async(&mut store, &component, &linker)
        .await
        .map_err(into_anyhow)?;

    let guest_result = module
        .et_ws_wasi_entry()
        .call_run(&mut store)
        .await
        .map_err(into_anyhow)?;

    guest_result.map_err(|e| anyhow::anyhow!("module run() returned err: {e}"))
}
