use std::path::PathBuf;

use serde::Deserialize;
use serde_default::DefaultFromSerde;

use crate::args::executable_name;
use crate::auth::BasicAuth;
use crate::ports::Services;

/// Localhost address 127.0.0.1 .
pub const LOCALHOST: &str = "127.0.0.1";

/// Helper to find repository root.
#[expect(clippy::missing_panics_doc)]
#[expect(clippy::unwrap_used)]
#[must_use]
pub fn get_project_root() -> PathBuf {
    match lets_find_up::find_up(".taplo.toml") {
        Ok(Some(mut path)) => {
            assert!(path.pop(), "Failed to drop the filename");
            path
        }
        Ok(None) => std::env::current_dir().unwrap(),
        Err(err) => {
            log::error!("{err}");
            std::env::current_dir().unwrap()
        }
    }
}

/// Returns the default module search paths for ws-server.
///
/// Includes the standard workspace paths and any npm packages installed via mise.
#[must_use]
pub fn default_modules_folders() -> Vec<PathBuf> {
    let project_root = get_project_root();
    let mut paths = vec![
        project_root.join("services/ws-server/static"),
        project_root.join("services/ws-wasm-agent"),
        project_root.join("data/model-modules"),
        project_root.join("services/ws-modules"),
    ];
    if let Some(p) = mise_npm_modules_path("onnxruntime-web") {
        paths.push(p);
    }
    if let Some(p) = mise_npm_modules_path("pyodide") {
        paths.push(p);
    }
    paths
}

/// Returns the install path for a `mise` tool, e.g. `mise where npm:onnxruntime-web`.
#[must_use]
pub fn mise_where(tool: &str) -> Option<PathBuf> {
    let output = std::process::Command::new("mise").args(["where", tool]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let s = std::str::from_utf8(&output.stdout).ok()?;
    let p = PathBuf::from(s.trim());
    p.is_dir().then_some(p)
}

/// Returns the `lib/node_modules` path within a mise npm tool install root.
///
/// This directory contains all npm packages installed by mise and can be used
/// as a modules search path.
#[must_use]
pub fn mise_npm_modules_path(package: &str) -> Option<PathBuf> {
    let p = mise_where(&format!("npm:{package}"))?.join("lib/node_modules");
    p.is_dir().then_some(p)
}

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
