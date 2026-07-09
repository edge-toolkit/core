//! Environment-driven configuration for the WASI runner.
//!
//! Deserialised from the process environment via `serde-env`. The `RUNNER_*`
//! and `WS_*` vars are parsed by the shared [`RunnerConfig`] / [`WsConfig`]
//! structs from `et-ws-runner-common`; `OTLP_*` populates [`OtlpConfig`].

use edge_toolkit::config::OtlpConfig;
use et_ws_runner_common::config::{RunnerConfig, WsConfig};
use serde::Deserialize;

/// WASI-runner configuration sourced from the environment.
#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct Config {
    /// `RUNNER_*` settings (`RUNNER_MODULE`, `RUNNER_TIMEOUT`).
    pub runner: RunnerConfig,
    /// `WS_*` settings (`WS_SERVER_URL`).
    #[serde(default)]
    pub ws: WsConfig,
    /// OpenTelemetry config, from the `OTLP_*` env vars; `None` logs to stderr.
    #[serde(default)]
    pub otlp: Option<OtlpConfig>,
    /// When `ET_TEST_COVERAGE=true`, preopen a `/cov` dir for instrumented guests to write their minicov
    /// `.profraw` into (collected by the wasi-cov task into the combined Rust coverage). Test-only, defaults off.
    #[serde(default)]
    pub et_test_coverage: bool,
}
