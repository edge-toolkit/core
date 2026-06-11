//! Verifies serde-env maps the shared `RUNNER_*` / `WS_*` env vars onto the nested config structs.
//!
//! Covers humantime `RUNNER_TIMEOUT` parsing and the defaults applied when a
//! variable is absent.
#![cfg(test)]
#![expect(
    clippy::expect_used,
    clippy::duration_suboptimal_units,
    reason = "test code: panics carry context, and exact second counts mirror the parsed inputs"
)]

use std::time::Duration;

use et_ws_runner_common::config::{RunnerConfig, WsConfig};
use serde::Deserialize;

/// Mirrors how each runner nests the shared structs under `runner` / `ws`.
#[derive(Debug, Deserialize)]
struct Config {
    runner: RunnerConfig,
    #[serde(default)]
    ws: WsConfig,
}

#[test]
fn maps_runner_and_ws_env_vars() {
    let config: Config = serde_env::from_iter([
        ("RUNNER_MODULE", "et-ws-data1"),
        ("RUNNER_TIMEOUT", "3m"),
        ("WS_SERVER_URL", "ws://example:9000/ws"),
    ])
    .expect("parse env");

    assert_eq!(config.runner.module, "et-ws-data1");
    assert_eq!(config.runner.timeout, Some(Duration::from_secs(180)));
    assert_eq!(config.ws.server_url, "ws://example:9000/ws");
}

#[test]
fn humantime_seconds_suffix_parses() {
    let config: Config = serde_env::from_iter([("RUNNER_MODULE", "m"), ("RUNNER_TIMEOUT", "120s")]).expect("parse env");

    assert_eq!(config.runner.timeout, Some(Duration::from_secs(120)));
}

#[test]
fn absent_optionals_default() {
    let config: Config = serde_env::from_iter([("RUNNER_MODULE", "m")]).expect("parse env");

    assert_eq!(config.runner.timeout, None);
    assert!(config.ws.server_url.starts_with("ws://localhost:"));
}

#[test]
fn missing_required_module_errors() {
    let result: Result<Config, _> = serde_env::from_iter([("WS_SERVER_URL", "ws://h/ws")]);

    assert!(result.is_err(), "RUNNER_MODULE is required");
}
