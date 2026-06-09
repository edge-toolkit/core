//! Native runner that executes browser-targeted ws-modules inside an embedded
//! Deno runtime (V8). Counterpart to `et-ws-wasi-runner`: that crate runs WASI
//! components inside wasmtime; this crate runs the JavaScript entry points
//! (wasm-bindgen glue, Pyodide shims, Dart/Zig/Java shims) that normally load
//! in a real browser.
//!
//! The runner fetches `package.json` from the ws-server, downloads the `main`
//! JS file, and evaluates it inside a Deno `JsRuntime` equipped with the
//! standard web platform extensions (fetch, `WebSocket`, `WebStorage`, timers,
//! crypto, WebGPU).

mod error;
mod runtime;

use futures_util::StreamExt as _;

pub use crate::error::RunnerError;

/// Convert a `ws://host[:port]/ws` URL to its `http://host[:port]` HTTP base
/// (or `wss://` -> `https://`). Returns `None` if `ws_url` is not a websocket
/// URL.
#[must_use]
pub fn derive_http_base(ws_url: &str) -> Option<String> {
    let (scheme, rest) = if let Some(suffix) = ws_url.strip_prefix("wss://") {
        ("https", suffix)
    } else if let Some(suffix) = ws_url.strip_prefix("ws://") {
        ("http", suffix)
    } else {
        return None;
    };
    let host_port = rest.strip_suffix("/ws").unwrap_or(rest);
    Some(format!("{scheme}://{host_port}"))
}

/// Download, prepare, and run the browser-targeted JS module for `module_name`.
///
/// The ws-server must be running and serving modules at the derived HTTP base.
#[expect(
    clippy::future_not_send,
    reason = "MainWorker is !Send; the caller must use a current_thread tokio runtime"
)]
pub async fn run_module(module_name: &str, ws_url: &str) -> Result<(), RunnerError> {
    // Ensure a rustls crypto provider is installed (needed by deno_tls/deno_fetch).
    // MainWorker bootstrap installs one but the workspace `et-rest-client` we
    // use ahead of MainWorker (to fetch package.json) also wants a provider, so
    // install eagerly here.
    let _ignore = deno_runtime::deno_tls::rustls::crypto::aws_lc_rs::default_provider().install_default();
    let http_base = derive_http_base(ws_url).ok_or_else(|| RunnerError::InvalidWsUrl {
        ws_url: ws_url.to_string(),
    })?;

    // 1. Fetch package.json to find the main JS entry point.
    let rest = et_rest_client::Client::new(&http_base);
    let pkg_stream = rest.get_module_file(module_name, "package.json").await?.into_inner();
    let pkg_bytes = collect_byte_stream(pkg_stream).await?;
    let pkg: serde_json::Value = serde_json::from_slice(&pkg_bytes)?;

    let main = pkg
        .get("main")
        .and_then(|value| value.as_str())
        .ok_or_else(|| RunnerError::PackageJsonMissingMain {
            module: module_name.to_string(),
        })?;

    let module_base_url = format!("{http_base}/modules/{module_name}");
    let entry_url = format!("{module_base_url}/{main}");

    tracing::info!(%entry_url, "running module JS");

    // 2. Create and run the Deno runtime.
    runtime::run_js_module(&entry_url, &http_base, ws_url, rest).await?;
    Ok(())
}

/// Drain a progenitor `ByteStream` into a `Vec<u8>`. The `?` on `chunk`
/// converts `reqwest::Error` to `et_rest_client::Error` via `From` so the
/// caller doesn't need a separate variant for the streaming-error type.
#[expect(
    clippy::single_call_fn,
    reason = "split out for readability; the named helper documents what `chunk?` propagates"
)]
async fn collect_byte_stream(mut stream: et_rest_client::ByteStream) -> Result<Vec<u8>, et_rest_client::Error<()>> {
    let mut buf = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}
