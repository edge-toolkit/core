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
    /// Runtime activation of the browser-module coverage capture, read from `ET_TEST_COVERAGE`.
    ///
    /// Only present under the `coverage` cargo feature -- the capture code is compiled in only there. When the
    /// feature is on, `ET_TEST_COVERAGE=true` makes each module PUT its minicov `.profraw` / coverage.py data to
    /// its own storage bucket for the web-runner test to gather; `false` (the default) leaves the compiled-in
    /// capture inert. serde-env parses the value with `str::parse::<bool>`, so it must be `true`/`false`.
    #[cfg(feature = "coverage")]
    #[serde(default)]
    pub et_test_coverage: bool,
}
