//! Smoke tests for the URL-handling helpers in `et-ws-wasi-runner`.
//!
//! Full integration with a real ws-server lives outside this crate (the
//! parent workspace's `et-ws-test-server` can't be pulled in here -- see
//! `Cargo.toml`). When you want to run a module end-to-end:
//!     mise run ws-server      # in one terminal
//!     mise run ws-wasi-runner # in another, with RUNNER_MODULE=wasi-graphics-info.

#![cfg(test)]

use et_ws_wasi_runner::{RunnerError, derive_http_base};

#[test]
fn derive_http_base_strips_ws_suffix() {
    for (input, expected) in [
        ("ws://localhost:8080/ws", "http://localhost:8080"),
        ("wss://example.com/ws", "https://example.com"),
        ("ws://10.0.0.1:9000", "http://10.0.0.1:9000"),
    ] {
        let actual = derive_http_base(input);
        assert!(
            matches!(&actual, Ok(base) if base == expected),
            "derive_http_base({input:?}): expected Ok({expected:?}), got {actual:?}"
        );
    }
}

#[test]
fn derive_http_base_rejects_non_ws_schemes() {
    for bad in ["http://localhost:8080", "not-a-url"] {
        let actual = derive_http_base(bad);
        assert!(
            matches!(&actual, Err(RunnerError::InvalidWsUrl { ws_url }) if ws_url == bad),
            "derive_http_base({bad:?}): expected Err(InvalidWsUrl), got {actual:?}"
        );
    }
}
