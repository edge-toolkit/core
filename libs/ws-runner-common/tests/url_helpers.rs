//! Smoke tests for the shared runner URL helper.

#![cfg(test)]

use et_ws_runner_common::{BootstrapError, derive_http_base};

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
            matches!(&actual, Err(BootstrapError::InvalidWsUrl { ws_url }) if ws_url == bad),
            "derive_http_base({bad:?}): expected Err(InvalidWsUrl), got {actual:?}"
        );
    }
}
