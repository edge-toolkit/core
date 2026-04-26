use std::path::PathBuf;

use edge_toolkit::config::OtlpConfig;
pub use et_modules_service::ModulesConfig;
pub use et_storage_service::StorageConfig;
use serde::Deserialize;
use serde_default::DefaultFromSerde;
use serde_inline_default::serde_inline_default;

/// TLS certificate and key paths.
#[serde_inline_default]
#[derive(Clone, Debug, DefaultFromSerde, Deserialize)]
pub struct TlsConfig {
    #[serde_inline_default(PathBuf::from("cert.pem"))]
    pub cert_file: PathBuf,
    #[serde_inline_default(PathBuf::from("key.pem"))]
    pub key_file: PathBuf,
}

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
    /// TLS config.
    #[serde(default)]
    pub tls: TlsConfig,
}
