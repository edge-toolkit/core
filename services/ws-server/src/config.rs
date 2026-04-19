use edge_toolkit::config::OtlpConfig;
use serde::Deserialize;
use serde_default::DefaultFromSerde;

/// Application environment variables and config.
#[derive(Clone, Debug, DefaultFromSerde, Deserialize)]
pub struct Config {
    /// OpenTelemetry config.
    #[serde(default)]
    pub otlp: Option<OtlpConfig>,
}
