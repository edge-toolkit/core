//! The WASI runner's environment configuration.
//!
//! It lives beside the shared settings rather than in `et-ws-wasi-runner`, so documenting it needs no wasmtime build.

use crate::runner_config;

runner_config! {
/// WASI-runner configuration sourced from the environment.
pub struct Config {
    /// Runtime activation of the guest-coverage preopen, read from `ET_TEST_COVERAGE`.
    ///
    /// Only present under the `coverage` cargo feature -- the preopen code is compiled in only there. When the
    /// feature is on, `ET_TEST_COVERAGE=true` preopens a `/cov` dir for instrumented guests to write their minicov
    /// `.profraw` into (collected by the wasi-cov task into the combined Rust coverage); `false` (the default)
    /// leaves the compiled-in preopen inert. Left out of the env schema, so a runner's HELP.md documents only what
    /// every build reads.
    #[cfg(feature = "coverage")]
    #[cfg_attr(feature = "env-schema", schemars(skip))]
    #[serde(default)]
    pub et_test_coverage: bool,
}
}
