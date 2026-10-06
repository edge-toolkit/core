//! The pyo3 runner's environment configuration.
//!
//! It lives beside the shared settings rather than in `et-ws-pyo3-runner`, so documenting it needs no Python build.

use std::path::PathBuf;

use serde::Deserialize;

use crate::runner_config;

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
#[cfg_attr(feature = "env-schema", derive(schemars::JsonSchema))]
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
