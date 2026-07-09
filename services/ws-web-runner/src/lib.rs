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

use std::time::Duration;

use et_ws_runner_common::{derive_http_base, fetch_main_field};

pub mod config;
mod error;
mod runtime;

pub use crate::error::RunnerError;

/// Download, prepare, and run the browser-targeted JS module for `module_name`.
///
/// The ws-server must be running and serving modules at the derived HTTP base.
#[expect(
    clippy::future_not_send,
    reason = "MainWorker is !Send; the caller must use a current_thread tokio runtime"
)]
pub async fn run_module(module_name: &str, ws_url: &str, pycov: bool) -> Result<(), RunnerError> {
    // Ensure a rustls crypto provider is installed (needed by deno_tls/deno_fetch).
    // MainWorker bootstrap installs one but the workspace `et-rest-client` we
    // use ahead of MainWorker (to fetch package.json) also wants a provider, so
    // install eagerly here.
    let _ignore = deno_runtime::deno_tls::rustls::crypto::aws_lc_rs::default_provider().install_default();
    let http_base = derive_http_base(ws_url)?;

    let rest = build_rest_client(&http_base)?;
    let main = fetch_main_field(&rest, module_name).await?;

    let module_base_url = format!("{http_base}/modules/{module_name}");
    let entry_url = format!("{module_base_url}/{main}");

    tracing::info!(%entry_url, "running module JS");

    runtime::run_js_module(&entry_url, &http_base, ws_url, rest, pycov).await?;
    Ok(())
}

/// Build the REST client with a reqwest retry policy that replays transport-
/// level send failures.
///
/// The pooled keep-alive race: the ws-server can close an idle connection while
/// the slow `MainWorker` bootstrap runs, so the next `send()` fails with "error
/// sending request". reqwest's default `ProtocolNacks` policy does NOT cover
/// this -- it only retries h2 `REFUSED_STREAM` / h3 timeouts, and we build
/// reqwest without the `http2` feature, so it's a no-op for these h1 fetches.
/// So classify any transport error (a send that produced no response) as
/// retryable, scoped to the ws-server host, with no budget so the idempotent,
/// low-volume module GETs always get their retry.
#[expect(
    clippy::single_call_fn,
    clippy::result_large_err,
    reason = "split out of run_module for readability; RunnerError::Common wraps a ~136 B BootstrapError"
)]
fn build_rest_client(http_base: &str) -> Result<et_rest_client::Client, RunnerError> {
    let host = reqwest::Url::parse(http_base)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_default();
    let retry = reqwest::retry::for_host(host).no_budget().classify_fn(|req_rep| {
        if req_rep.error().is_some() {
            req_rep.retryable()
        } else {
            req_rep.success()
        }
    });
    let dur = Duration::from_secs(15);
    let client = reqwest::Client::builder()
        .connect_timeout(dur)
        .timeout(dur)
        .retry(retry)
        .build()?;
    Ok(et_rest_client::Client::new_with_client(http_base, client))
}
