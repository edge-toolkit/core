//! Smoke tests for the URL-handling helpers in `et-ws-wasi-runner`.
//!
//! Full integration with a real ws-server lives outside this crate (the
//! parent workspace's `et-ws-test-server` can't be pulled in here — see
//! `Cargo.toml`). When you want to run a module end-to-end:
//!     mise run ws-server      # in one terminal
//!     mise run ws-wasi-runner # in another, with RUNNER_MODULE=wasi-graphics-info

#![cfg(test)]

use et_ws_wasi_runner::derive_http_base;

#[test]
fn derive_http_base_strips_ws_suffix() {
    assert_eq!(
        derive_http_base("ws://localhost:8080/ws"),
        Some("http://localhost:8080".to_string())
    );
    assert_eq!(
        derive_http_base("wss://example.com/ws"),
        Some("https://example.com".to_string())
    );
    assert_eq!(
        derive_http_base("ws://10.0.0.1:9000"),
        Some("http://10.0.0.1:9000".to_string())
    );
}

#[test]
fn derive_http_base_rejects_non_ws_schemes() {
    assert_eq!(derive_http_base("http://localhost:8080"), None);
    assert_eq!(derive_http_base("not-a-url"), None);
}
