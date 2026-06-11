//! Environment-driven configuration for the web runner.
//!
//! Deserialised from the process environment via `serde-env`. The `RUNNER_*`
//! and `WS_*` vars are parsed by the shared [`RunnerConfig`] / [`WsConfig`]
//! structs from `et-ws-runner-common`.

use et_ws_runner_common::config::{RunnerConfig, WsConfig};
use serde::Deserialize;

/// Web-runner configuration sourced from the environment.
#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct Config {
    /// `RUNNER_*` settings (`RUNNER_MODULE`, `RUNNER_TIMEOUT`).
    pub runner: RunnerConfig,
    /// `WS_*` settings (`WS_SERVER_URL`).
    #[serde(default)]
    pub ws: WsConfig,
}
