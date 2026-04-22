use std::path::PathBuf;

use edge_toolkit::config::OtlpConfig;
use serde::Deserialize;
use serde_default::DefaultFromSerde;
use serde_inline_default::serde_inline_default;

/// Default modules directory.
#[must_use]
pub fn default_modules_folder() -> Vec<std::path::PathBuf> {
    let project_root = edge_toolkit::config::get_project_root();
    vec![
        project_root.join("services/ws-wasm-agent"),
        project_root.join("data").join("model-modules"),
        project_root.join("services").join("ws-modules"),
    ]
}

/// Application environment variables and config.
#[serde_inline_default]
#[derive(Clone, Debug, DefaultFromSerde, Deserialize)]
pub struct ModulesConfig {
    #[serde(default = "default_modules_folder")]
    pub paths: Vec<PathBuf>,
}

/// Application environment variables and config.
#[derive(Clone, Debug, DefaultFromSerde, Deserialize)]
pub struct Config {
    /// OpenTelemetry config.
    #[serde(default)]
    pub otlp: Option<OtlpConfig>,
    /// Modules config.
    #[serde(default)]
    pub modules: ModulesConfig,
}
