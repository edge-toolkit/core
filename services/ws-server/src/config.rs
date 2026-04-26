use edge_toolkit::config::OtlpConfig;
pub use et_modules_service::ModulesConfig;
pub use et_storage_service::StorageConfig;
use serde::Deserialize;
use serde_default::DefaultFromSerde;

/// Application config shared across ws-server services.
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
