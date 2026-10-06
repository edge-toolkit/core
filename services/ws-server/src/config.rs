#[cfg(feature = "tls")]
use std::path::PathBuf;

#[cfg(feature = "otlp")]
use edge_toolkit::config::OtlpConfig;
#[cfg(feature = "modules")]
pub use et_modules_service::ModulesConfig;
#[cfg(feature = "storage")]
pub use et_storage_service::StorageConfig;
pub use et_ws_service::WsConfig;
use serde::Deserialize;
use serde_default::DefaultFromSerde;
#[cfg(feature = "tls")]
use serde_inline_default::serde_inline_default;

/// TLS certificate and key paths.
#[cfg(feature = "tls")]
#[serde_inline_default]
#[derive(Clone, Debug, DefaultFromSerde, Deserialize)]
#[cfg_attr(feature = "env-schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct TlsConfig {
    #[serde_inline_default(PathBuf::from("cert.pem"))]
    pub cert_file: PathBuf,
    #[serde_inline_default(PathBuf::from("key.pem"))]
    pub key_file: PathBuf,
}

/// Network-address selection for the startup banner and QR code.
#[derive(Clone, Debug, DefaultFromSerde, Deserialize)]
#[cfg_attr(feature = "env-schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct NetConfig {
    /// Name of the interface whose address the startup banner and QR code should advertise first.
    ///
    /// Set it to `bridge100` when the machine shares its internet connection as a Wi-Fi hotspot: macOS
    /// Internet Sharing puts the hotspot's gateway address there, and it is the only address the devices
    /// scanning the QR code can reach. Naming an interface makes it a hard requirement -- startup fails
    /// if it is absent or holds no usable IPv4 address. When unset, ranking is automatic: `en*` NICs
    /// first, then `bridge*`, then everything else.
    #[serde(default)]
    pub log_interface: Option<String>,
}

/// Application config shared across ws-server services.
#[derive(Clone, Debug, DefaultFromSerde, Deserialize)]
#[cfg_attr(feature = "env-schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Config {
    /// OpenTelemetry config.
    #[cfg(feature = "otlp")]
    #[serde(default)]
    pub otlp: Option<OtlpConfig>,
    /// Modules config.
    #[cfg(feature = "modules")]
    #[serde(default)]
    pub modules: ModulesConfig,
    /// Startup banner / QR code address selection.
    /// `serde-env` maps the inner fields as `NET_*`, e.g. `NET_PREFER_HOTSPOT_IP`.
    #[serde(default)]
    pub net: NetConfig,
    /// Storage config.
    #[cfg(feature = "storage")]
    #[serde(default)]
    pub storage: StorageConfig,
    /// TLS config.
    #[cfg(feature = "tls")]
    #[serde(default)]
    pub tls: TlsConfig,
    /// WebSocket hub config (frame limits, etc.).
    /// `serde-env` maps the inner fields as `WS_*`, e.g. `WS_MAX_FRAME_SIZE`.
    #[serde(default)]
    pub ws: WsConfig,
}
