//! Environment-driven configuration for the WASI runner.
//!
//! Deserialised from the process environment via `serde-env`. The fields every
//! runner reads come from [`et_ws_runner_common::runner_config`], which supplies
//! the `RUNNER_*`, `WS_*` and `OTLP_*` groups; what is written out below is only
//! what this runner adds to them.

use et_ws_runner_common::runner_config;

runner_config! {
/// WASI-runner configuration sourced from the environment.
pub struct Config {
    /// Runtime activation of the guest-coverage preopen, read from `ET_TEST_COVERAGE`.
    ///
    /// Only present under the `coverage` cargo feature -- the preopen code is compiled in only there. When the
    /// feature is on, `ET_TEST_COVERAGE=true` preopens a `/cov` dir for instrumented guests to write their minicov
    /// `.profraw` into (collected by the wasi-cov task into the combined Rust coverage); `false` (the default)
    /// leaves the compiled-in preopen inert.
    #[cfg(feature = "coverage")]
    #[serde(default)]
    pub et_test_coverage: bool,
}
}
