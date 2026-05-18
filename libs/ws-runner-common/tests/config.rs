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

// --- WS_CONNECT_ACK_TIMEOUT, via the real `from_env` path (temp-env) --------
//
// Unset -> 5s default; a humantime value -> that duration. A *blank* value is
// dropped by serde-env (it filters empty-valued vars), so it is
// indistinguishable from unset and falls back to the default -- which is why
// the timeout can't be disabled with an empty env var, and why the field uses
// the plain `#[serde(default, with = "humantime_serde")]` form with no custom
// blank handling.

#[derive(Debug, Deserialize)]
struct WsOnly {
    #[serde(default)]
    ws: WsConfig,
}

fn load_ws() -> WsConfig {
    serde_env::from_env::<WsOnly>().expect("parse WsConfig from env").ws
}

#[test]
fn connect_ack_timeout_absent_defaults_to_5s() {
    temp_env::with_var_unset("WS_CONNECT_ACK_TIMEOUT", || {
        assert_eq!(load_ws().connect_ack_timeout, Some(Duration::from_secs(5)));
    });
}

#[test]
fn connect_ack_timeout_parses_humantime() {
    temp_env::with_var("WS_CONNECT_ACK_TIMEOUT", Some("1m30s"), || {
        assert_eq!(load_ws().connect_ack_timeout, Some(Duration::from_secs(90)));
    });
}

#[test]
fn connect_ack_timeout_none_sentinel_disables() {
    for sentinel in ["none", "off", "disabled", "NONE", "Off"] {
        temp_env::with_var("WS_CONNECT_ACK_TIMEOUT", Some(sentinel), || {
            assert_eq!(
                load_ws().connect_ack_timeout,
                None,
                "sentinel {sentinel:?} should disable"
            );
        });
    }
}

#[test]
fn blank_env_var_is_filtered_by_serde_env_so_default_applies() {
    // serde-env drops empty-valued vars, so a blank value behaves exactly like
    // an unset one (the 5s default), NOT as a way to disable the timeout --
    // which is why disabling uses the `none` / `off` sentinel above.
    temp_env::with_var("WS_CONNECT_ACK_TIMEOUT", Some(""), || {
        assert_eq!(load_ws().connect_ack_timeout, Some(Duration::from_secs(5)));
    });
}
