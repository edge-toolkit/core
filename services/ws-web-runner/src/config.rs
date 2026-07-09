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
    /// Optional V8 flags from `V8_FLAGS`, applied via `v8::V8::set_flags_from_string` before the
    /// runtime initialises. Used to select the WASM compile tier when debugging the gnullvm
    /// dotnet-data1 crash (e.g. `--no-liftoff`, `--liftoff-only`, `--jitless`).
    #[serde(default)]
    pub v8_flags: Option<String>,
    /// When `ET_TEST_COVERAGE=true`, the Pyodide module shims collect coverage.py data and PUT it to ws-server
    /// storage for the web-runner integration test to gather into the combined Python coverage report. Test-only;
    /// defaults off (serde-env parses the value with `str::parse::<bool>`, so it must be `true`/`false`), and in
    /// production the coverage shim then does nothing.
    #[serde(default)]
    pub et_test_coverage: bool,
}
