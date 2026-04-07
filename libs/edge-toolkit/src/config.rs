use serde::Deserialize;
use serde_default::DefaultFromSerde;

use crate::args::executable_name;
use crate::auth::BasicAuth;
use crate::ports::Services;

/// Localhost address 127.0.0.1 .
pub const LOCALHOST: &str = "127.0.0.1";

/// Default port for the otlp http collector.
#[must_use]
const fn default_otlp_collector_port() -> u16 {
    Services::OtlpCollector.port()
}

/// Default url for the otlp collector. This is the tracing endpoint path for OpenObserve trace collection.
#[must_use]
pub fn default_otlp_collector_url() -> String {
    format!("http://{LOCALHOST}:{}/api/default/v1", default_otlp_collector_port())
}

/// Default service label name for use in OpenTelemetry.
///
/// Removes "-server" suffix from the invoked executable name if present,
/// such as binary name `et-ws-server`.
#[must_use]
pub fn default_trace_service_label() -> String {
    executable_name().replace("-server", "")
}

/// OTLP message data protocol.
///
/// Binary is more compact and efficient, while JSON is more human-readable and easier to debug.
#[expect(clippy::exhaustive_enums)]
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub enum OtlpProtocol {
    /// Binary messages.
    #[default]
    Binary,
    /// JSON messages.
    JSON,
}

/// OpenTelemetry service config.
#[derive(Clone, Debug, DefaultFromSerde, Deserialize)]
#[non_exhaustive]
pub struct OtlpConfig {
    /// OpenTelemetry collector URL.
    #[serde(default = "default_otlp_collector_url")]
    pub collector_url: String,
    /// OpenTelemetry protocol.
    #[serde(default)]
    pub protocol: OtlpProtocol,
    /// OpenTelemetry service label.
    #[serde(default = "default_trace_service_label")]
    pub service_label: String,
    /// OpenTelemetry HTTP basic auth.
    pub auth: Option<BasicAuth>,
}
