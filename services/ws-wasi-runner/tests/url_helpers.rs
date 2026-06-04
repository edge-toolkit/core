//! Smoke tests for the URL-handling helpers in `et-ws-wasi-runner`.
//!
//! Full integration with a real ws-server lives outside this crate (the
//! parent workspace's `et-ws-test-server` can't be pulled in here — see
//! `Cargo.toml`). When you want to run a module end-to-end:
//!     mise run ws-server      # in one terminal
//!     mise run ws-wasi-runner # in another, with RUNNER_MODULE=wasi-graphics-info.

#![cfg(test)]

use et_ws_wasi_runner::{RunnerError, derive_http_base};

#[test]
fn derive_http_base_strips_ws_suffix() {
    assert_eq!(
        derive_http_base("ws://localhost:8080/ws").unwrap(),
        "http://localhost:8080"
    );
    assert_eq!(
        derive_http_base("wss://example.com/ws").unwrap(),
        "https://example.com"
    );
    assert_eq!(
        derive_http_base("ws://10.0.0.1:9000").unwrap(),
        "http://10.0.0.1:9000"
    );
}

#[test]
fn derive_http_base_rejects_non_ws_schemes() {
    for bad in ["http://localhost:8080", "not-a-url"] {
        match derive_http_base(bad) {
            Err(RunnerError::InvalidWsUrl { ws_url }) => assert_eq!(ws_url, bad),
            other => panic!("expected InvalidWsUrl for {bad:?}, got {other:?}"),
        }
    }
}
