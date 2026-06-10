//! Native runner that executes browser-targeted ws-modules under embedded Deno.
//!
//! Counterpart to `et-ws-wasi-runner` (which runs WASI components inside
//! wasmtime); this crate runs the JavaScript entry points (wasm-bindgen glue,
//! Pyodide shims, Dart/Zig/Java shims) that normally load in a real browser.
//!
//! The runner fetches `package.json` from the ws-server, downloads the `main`
//! JS file, and evaluates it inside a Deno `JsRuntime` equipped with the
//! standard web platform extensions (fetch, `WebSocket`, `WebStorage`, timers,
//! crypto, WebGPU).

mod error;
mod runtime;

use et_ws_runner_common::{derive_http_base, fetch_main_field};

pub use crate::error::RunnerError;

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
    let http_base = derive_http_base(ws_url)?;

    let rest = et_rest_client::Client::new(&http_base);
    let main = fetch_main_field(&rest, module_name).await?;

    let module_base_url = format!("{http_base}/modules/{module_name}");
    let entry_url = format!("{module_base_url}/{main}");

    tracing::info!(%entry_url, "running module JS");

    runtime::run_js_module(&entry_url, &http_base, ws_url, rest).await?;
    Ok(())
}
