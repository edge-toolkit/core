//! Helpers shared by the two ws-module runners.
//!
//! `et-ws-wasi-runner` (WASI components under wasmtime) and `et-ws-web-runner`
//! (browser-targeted JS under Deno) both talk to the same ws-server REST
//! surface to bootstrap a module: derive the HTTP base from the WebSocket URL,
//! drain streamed responses, and read the `main` entry from `package.json`.
//! Those steps were duplicated in each crate; they live here so there is one
//! implementation to keep in sync with the server.

// `BootstrapError` is large because `et_rest_client::Error<()>` carries an
// inline `reqwest::Response` (~136 B). Boxing would cost a `From` impl per
// variant; not worth it for these one-shot runner helpers (the two runner
// crates carry the same expectation for the same reason).
#![expect(
    clippy::result_large_err,
    reason = "et_rest_client::Error<()> dominates the footprint; boxing would force per-variant From impls"
)]

use futures_util::StreamExt as _;
use thiserror::Error;

/// Errors produced while bootstrapping a module from the ws-server.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BootstrapError {
    /// `ws_url` was not a `ws://` / `wss://` URL, so no HTTP base could be derived.
    #[error("could not derive HTTP base from WS_SERVER_URL={ws_url}")]
    InvalidWsUrl { ws_url: String },

    /// A REST request to the ws-server failed.
    #[error(transparent)]
    Rest(#[from] et_rest_client::Error<()>),

    /// Streaming a response body chunk from the ws-server failed.
    ///
    /// `ByteStream` chunks surface as `reqwest::Error`, distinct from the typed `Rest` arm.
    #[error(transparent)]
    Stream(#[from] reqwest::Error),

    /// A module's `package.json` was not valid JSON.
    #[error(transparent)]
    PackageJsonInvalid(#[from] serde_path_to_error::Error<serde_json::Error>),

    /// A module's `package.json` parsed but had no `main` field.
    #[error("module {module} package.json missing `main` field")]
    PackageJsonMissingMain { module: String },
}

/// Derive a module's HTTP base URL from the ws-server WebSocket URL.
///
/// Maps the scheme (`ws://` -> `http://`, `wss://` -> `https://`) and strips a
/// trailing `/ws` path, e.g. `ws://host:8080/ws` -> `http://host:8080`.
///
/// # Errors
/// Returns [`BootstrapError::InvalidWsUrl`] if `ws_url` is not a `ws://` /
/// `wss://` URL.
pub fn derive_http_base(ws_url: &str) -> Result<String, BootstrapError> {
    let (scheme, rest) = if let Some(suffix) = ws_url.strip_prefix("wss://") {
        ("https", suffix)
    } else if let Some(suffix) = ws_url.strip_prefix("ws://") {
        ("http", suffix)
    } else {
        return Err(BootstrapError::InvalidWsUrl {
            ws_url: ws_url.to_string(),
        });
    };
    let host_port = rest.strip_suffix("/ws").unwrap_or(rest);
    Ok(format!("{scheme}://{host_port}"))
}

/// Drain a progenitor `ByteStream` into a `Vec<u8>`.
///
/// # Errors
/// Returns [`BootstrapError::Stream`] if downloading a chunk fails.
pub async fn collect_byte_stream(mut stream: et_rest_client::ByteStream) -> Result<Vec<u8>, BootstrapError> {
    let mut buf = Vec::new();
    while let Some(chunk) = stream.next().await {
        buf.extend_from_slice(&chunk?);
    }
    Ok(buf)
}

/// Read a module's `main` entry-point filename from its `package.json`.
///
/// Fetches `package.json` from the ws-server and returns its `main` field,
/// which names the file the runner downloads next (a WASI component for the
/// wasi runner, a JS entry for the web runner).
///
/// # Errors
/// Returns [`BootstrapError::Rest`] / [`BootstrapError::Stream`] if the fetch
/// fails, [`BootstrapError::PackageJsonInvalid`] if the body is not valid JSON,
/// or [`BootstrapError::PackageJsonMissingMain`] if it has no `main` field.
#[tracing::instrument(name = "fetch_package_json", skip(client), err)]
pub async fn fetch_main_field(client: &et_rest_client::Client, module_name: &str) -> Result<String, BootstrapError> {
    let response = client.get_module_file(module_name, "package.json").await?;
    let bytes = collect_byte_stream(response.into_inner()).await?;
    let mut deserializer = serde_json::Deserializer::from_slice(&bytes);
    let pkg: serde_json::Value = serde_path_to_error::deserialize(&mut deserializer)?;
    pkg.get("main")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| BootstrapError::PackageJsonMissingMain {
            module: module_name.to_string(),
        })
}
