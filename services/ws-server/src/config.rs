use std::path::PathBuf;

use edge_toolkit::config::{OtlpConfig, default_modules_folders};
use serde::Deserialize;
use serde_default::DefaultFromSerde;
use serde_inline_default::serde_inline_default;

/// Default modules directory.
#[must_use]
pub fn default_storage_folder() -> std::path::PathBuf {
    let project_root = edge_toolkit::config::get_project_root();
    project_root.join("services/ws-server/storage")
}

/// Modules config.
#[serde_inline_default]
#[derive(Clone, Debug, DefaultFromSerde, Deserialize)]
pub struct ModulesConfig {
    #[serde(default = "default_modules_folders")]
    pub paths: Vec<PathBuf>,
    #[serde_inline_default(String::from("et-ws-server-static"))]
    pub root: String,
}

/// Storage config.
#[derive(Clone, Debug, DefaultFromSerde, Deserialize)]
pub struct StorageConfig {
    #[serde(default = "default_storage_folder")]
    pub path: PathBuf,
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
    /// Storage config.
    #[serde(default)]
    pub storage: StorageConfig,
}
