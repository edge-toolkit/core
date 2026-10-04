//! The web runner's environment configuration.
//!
//! It lives beside the shared settings rather than in `et-ws-web-runner`, so documenting it needs no Deno build.

use crate::runner_config;

runner_config! {
/// Web-runner configuration sourced from the environment.
pub struct Config {
    /// Optional V8 flags from `V8_FLAGS`, applied via `v8::V8::set_flags_from_string` before the runtime initialises.
    ///
    /// Used to select the WASM compile tier when debugging the gnullvm dotnet-data1 crash (e.g. `--no-liftoff`,
    /// `--liftoff-only`, `--jitless`).
    #[serde(default)]
    pub v8_flags: Option<String>,
    /// Runtime activation of the browser-module coverage capture, read from `ET_TEST_COVERAGE`.
    ///
    /// Only present under the `coverage` cargo feature -- the capture code is compiled in only there. When the feature
    /// is on, `ET_TEST_COVERAGE=true` makes each module PUT its minicov `.profraw` / coverage.py data to its own
    /// storage bucket for the web-runner test to gather; `false` (the default) leaves the compiled-in capture inert.
    /// serde-env parses the value with `str::parse::<bool>`, so it must be `true`/`false`. Left out of the env schema,
    /// so a runner's HELP.md documents only what every build reads.
    #[cfg(feature = "coverage")]
    #[cfg_attr(feature = "env-schema", schemars(skip))]
    #[serde(default)]
    pub et_test_coverage: bool,
}
}
