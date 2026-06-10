use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_default::DefaultFromSerde;
use serde_inline_default::serde_inline_default;

use crate::args::executable_name;
use crate::auth::BasicAuth;
use crate::ports::Services;

/// Localhost address 127.0.0.1 .
pub const LOCALHOST: &str = "127.0.0.1";

/// Helper to find repository root.
#[expect(clippy::missing_panics_doc, clippy::unwrap_used)]
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
        project_root.join("generated/python-ws"),
        project_root.join("generated/python-rest"),
    ];
    // Skip mise-managed module resolution when mise isn't on PATH: the
    // per-package "run `mise install ...`" warnings would just confuse a
    // deployment that has provisioned those modules some other way (or
    // doesn't serve them at all).
    if !mise_is_available() {
        return paths;
    }
    match mise_npm_modules_path("onnxruntime-web") {
        Some(path) => {
            log::info!("Resolved npm:onnxruntime-web modules path: {}", path.display());
            paths.push(path);
        }
        None => {
            log::warn!(
                "{}",
                concat!(
                    "npm:onnxruntime-web install path not found via `mise where` -- ",
                    "requests to /modules/onnxruntime-web/* will 404. ",
                    "Run `mise install npm:onnxruntime-web` and verify the package layout.",
                )
            );
        }
    }
    // Pyodide is installed from its GitHub release tarball (see `.mise.toml`),
    // not via `npm:pyodide`. mise's http backend extracts the archive flat,
    // so the install dir itself holds `package.json` + every wheel -- the
    // modules service treats the dir as a single module named "pyodide".
    // Fall back to the much smaller `npm:pyodide` install if the full
    // distribution isn't available: browser modules that only need pyodide's
    // runtime (no `micropip.install` of non-stdlib wheels) still work, and
    // contributors who don't need the full set can skip the 200 MB download.
    match mise_where("http:pyodide").or_else(|| mise_npm_modules_path("pyodide")) {
        Some(path) => {
            log::info!("Resolved pyodide modules path: {}", path.display());
            paths.push(path);
        }
        None => {
            log::warn!(
                "{}",
                concat!(
                    "pyodide install path not found via `mise where http:pyodide` or `mise where npm:pyodide` -- ",
                    "requests to /modules/pyodide/* will 404. Run `mise install` and verify the install.",
                )
            );
        }
    }
    paths
}

/// Returns `true` if the `mise` binary is reachable on `PATH`. A failed
/// `Command::output()` indicates the binary couldn't be spawned --
/// typically because it's not installed or the test is hiding `PATH`.
#[must_use]
pub fn mise_is_available() -> bool {
    std::process::Command::new("mise").arg("--version").output().is_ok()
}

/// Returns the install path for a `mise` tool, e.g. `mise where npm:onnxruntime-web`.
#[must_use]
pub fn mise_where(tool: &str) -> Option<PathBuf> {
    let output = std::process::Command::new("mise").args(["where", tool]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = std::str::from_utf8(&output.stdout).ok()?;
    let path = PathBuf::from(stdout.trim());
    path.is_dir().then_some(path)
}

/// Returns the directory containing `<package>` for an `npm:<package>` mise install.
///
/// I.e. the `node_modules` directory you'd point `MODULES_PATHS` at. Calls
/// `mise where npm:<package>` to find the install root, then delegates to
/// [`find_npm_modules_path_in`] to handle the per-backend layout differences.
/// Returns `None` if `mise where` fails or the package isn't present in any
/// supported layout.
#[must_use]
pub fn mise_npm_modules_path(package: &str) -> Option<PathBuf> {
    let install = mise_where(&format!("npm:{package}"))?;
    find_npm_modules_path_in(&install, package)
}

/// Pure-filesystem version of [`mise_npm_modules_path`]: given an
/// `<install>` root and a `<package>` name, return the `node_modules`
/// directory that contains `<package>`. Supports the mise npm backends:
///
/// 1. Classical npm/mise (Unix): `<install>/lib/node_modules/<package>`
/// 2. npm on Windows: `<install>/node_modules/<package>` (no `lib/` segment --
///    npm's global prefix layout differs by platform)
/// 3. aube backend: `<install>/global-aube/<hash>/node_modules/.aube/node_modules/<package>`
///
/// Tried in that order; returns `None` if no layout has the package.
#[must_use]
pub fn find_npm_modules_path_in(install: &Path, package: &str) -> Option<PathBuf> {
    let classical = install.join("lib/node_modules");
    if classical.join(package).is_dir() {
        return Some(classical);
    }

    let windows = install.join("node_modules");
    if windows.join(package).is_dir() {
        return Some(windows);
    }

    let aube_root = install.join("global-aube");
    if let Ok(entries) = std::fs::read_dir(&aube_root) {
        for entry in entries.flatten() {
            let node_modules = entry.path().join("node_modules/.aube/node_modules");
            if node_modules.join(package).is_dir() {
                return Some(node_modules);
            }
        }
    }

    None
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
#[serde_inline_default]
#[derive(Clone, Debug, DefaultFromSerde, Deserialize)]
#[non_exhaustive]
pub struct OtlpConfig {
    /// OpenTelemetry collector URL.
    #[serde_inline_default(format!("http://{LOCALHOST}:{}/api/default/v1", Services::OtlpCollector.port()))]
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
