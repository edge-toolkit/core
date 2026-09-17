//! Environment-driven configuration for the pyo3 runner.
//!
//! Deserialised from the process environment via `serde-env`. The fields every
//! runner reads come from [`et_ws_runner_common::runner_config`], so this runner
//! reads the same vars as the WASI and web runners; `PYO3_*` populates the
//! runner-specific bits written out below.
//!
//!   `RUNNER_MODULE`    (required) -- Python module name to import.
//!   `RUNNER_TIMEOUT`   (optional) -- wall-clock run limit, e.g. `120s`, `3m`.
//!   `WS_SERVER_URL`    (optional) -- defaults to the local insecure ws port.
//!   `OTLP_*`           (optional) -- populates `OtlpConfig`; absent logs to stderr.
//!   `PYO3_PYTHONPATH`  (optional) -- colon-separated paths prepended to `sys.path`.
//!   `PYO3_AGENT_ID`    (optional) -- request this `agent_id` on connect.

use std::path::PathBuf;

use et_ws_runner_common::runner_config;
use serde::Deserialize;

runner_config! {
    /// Configuration for the pyo3 runner, sourced from the environment.
    pub struct Config {
        /// `PYO3_*` settings unique to this runner.
        #[serde(default)]
        pub pyo3: Pyo3Config,
    }
}

/// Runner-specific `PYO3_*` settings with no shared equivalent.
#[derive(Clone, Debug, Default, Deserialize)]
#[non_exhaustive]
pub struct Pyo3Config {
    /// `PYO3_PYTHONPATH` -- colon-separated paths prepended to `sys.path`.
    ///
    /// Prepended before importing the module; empty by default.
    #[serde(default)]
    pub pythonpath: String,
    /// `PYO3_AGENT_ID` -- request this `agent_id` on connect; unset gets a fresh one.
    #[serde(default)]
    pub agent_id: Option<String>,
}

impl Pyo3Config {
    /// Split `PYO3_PYTHONPATH` into path entries, dropping empty segments.
    #[must_use]
    pub fn python_path(&self) -> Vec<PathBuf> {
        self.pythonpath
            .split(':')
            .filter(|segment| !segment.is_empty())
            .map(PathBuf::from)
            .collect()
    }
}
