use std::path::{Path, PathBuf};
use std::time::Duration;

use fs_err as fs;
use serde::Deserialize;
use serde_default::DefaultFromSerde;
use serde_inline_default::serde_inline_default;

use crate::args::executable_name;
use crate::auth::BasicAuth;
use crate::ports::Services;

/// Localhost address 127.0.0.1 .
pub const LOCALHOST: &str = "127.0.0.1";

/// Whether a config value names the "disabled" state: `none`, `off`, or
/// `disabled` (case-insensitive, surrounding whitespace ignored).
///
/// These sentinels let an `Option<_>` env-var field be set to `None`. A blank
/// value can't serve that role -- `serde-env` drops empty-valued vars, so a
/// blank var is indistinguishable from unset (both fall back to the default).
#[must_use]
pub fn is_disabled_sentinel(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.eq_ignore_ascii_case("none")
        || trimmed.eq_ignore_ascii_case("off")
        || trimmed.eq_ignore_ascii_case("disabled")
}

/// Deserialize `Option<T>` where a disable sentinel ([`is_disabled_sentinel`])
/// maps to `None` and any other value to `Some(T)` via `T`'s own `Deserialize`.
///
/// Generic over the inner type, for fields read from env vars via `serde-env`:
/// the value arrives as a string, so this works for any `T` whose `Deserialize`
/// accepts a string scalar (e.g. `bytesize::ByteSize`, `String`). `Duration`
/// is the exception -- its `Deserialize` isn't humantime -- so use
/// [`deserialize_optional_humantime`] for duration fields. Pair either with
/// `#[serde(default = "...")]` for the unset case.
///
/// # Errors
/// Returns the deserializer's error if the value is neither a sentinel nor a
/// valid `T`.
pub fn deserialize_optional<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    use serde::de::IntoDeserializer as _;

    let raw = <String as Deserialize>::deserialize(deserializer)?;
    if is_disabled_sentinel(&raw) {
        return Ok(None);
    }
    let inner: serde::de::value::StrDeserializer<'_, D::Error> = raw.trim().into_deserializer();
    T::deserialize(inner).map(Some)
}

/// [`deserialize_optional`] for `Option<Duration>` fields, parsing the value as
/// a humantime duration (e.g. `15s`, `1m30s`).
///
/// Separate from the generic [`deserialize_optional`] because `Duration`'s own
/// `Deserialize` expects a `{secs, nanos}` struct, not a humantime string.
///
/// # Errors
/// Returns the deserializer's error if the value is neither a sentinel nor a
/// valid humantime duration.
pub fn deserialize_optional_humantime<'de, D>(deserializer: D) -> Result<Option<Duration>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::IntoDeserializer as _;

    let raw = <String as Deserialize>::deserialize(deserializer)?;
    if is_disabled_sentinel(&raw) {
        return Ok(None);
    }
    let inner: serde::de::value::StrDeserializer<'_, D::Error> = raw.trim().into_deserializer();
    humantime_serde::deserialize(inner).map(Some)
}

/// Helper to find repository root.
///
/// This is the one sanctioned `current_dir()`.
#[expect(
    clippy::disallowed_methods,
    clippy::missing_panics_doc,
    clippy::unwrap_used,
    reason = "the one sanctioned current_dir() -- this helper is what the disallowed-methods ban points callers to"
)]
#[must_use]
pub fn get_project_root() -> PathBuf {
    et_path::find_project_root(&std::env::current_dir().unwrap())
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
    // Pyodide is installed from its GitHub release tarball (see
    // `.mise/config.python.toml`),
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

/// Guest languages mise loads via `MISE_ENV`.
///
/// Each variant maps to a `.mise/config.<env>.toml` file
/// (e.g. `Self::Python` -> `config.python.toml`) that adds that language's
/// toolchain on top of the always-loaded base `.mise/config.toml`. Mirrors
/// `ALL_LANGS` in `.mise/config.toml`; keep the two in sync.
///
/// `strum::IntoStaticStr` derives the canonical lowercase name used in
/// `MISE_ENV` and the config filename; `strum::EnumIter` enumerates all
/// variants in declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::IntoStaticStr, strum::EnumIter)]
#[strum(serialize_all = "lowercase")]
#[non_exhaustive]
pub enum Language {
    Dart,
    Dotnet,
    Java,
    Js,
    Python,
    Rust,
    Zig,
}

impl Language {
    /// Canonical lowercase name as used in `MISE_ENV` and the
    /// `config.<name>.toml` filename.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// Whether `MISE_ENV` loads the named language config.
///
/// `MISE_ENV` is a comma-separated list of guest-language envs (each adds a
/// `config.<env>.toml`). The unset and explicitly-empty cases mean different
/// things:
///
/// - **`MISE_ENV` unset** (no env var at all): typical local-dev state where
///   the developer's `mise install` covered everything; treat every language
///   as available.
/// - **`MISE_ENV=""`** (set, empty): CI/Docker explicitly narrowed the env
///   to "no guest languages". Every language returns false; tests that
///   depend on a guest-env tool skip cleanly.
///
/// Used to gate live-installed-tool tests: when CI narrows `MISE_ENV` for
/// faster feedback (e.g. `dotnet,rust`), tests that depend on a tool in a
/// dropped env (e.g. `http:pyodide` from the `python` env) can skip cleanly
/// instead of panicking.
///
/// When the language is absent (`MISE_ENV=""` or a list that omits it), a
/// skip line naming the language is emitted to stderr, so callers only need
/// to branch on the returned bool -- they don't repeat the message themselves.
#[must_use]
pub fn mise_env_includes(language: Language) -> bool {
    const MISE_ENV: &str = "MISE_ENV";
    let Ok(value) = std::env::var(MISE_ENV) else {
        return true;
    };
    let included = !value.is_empty() && value.split(',').any(|seg| seg.trim() == language.as_str());
    if !included {
        #[expect(
            clippy::print_stderr,
            reason = "intentional skip notice to stderr so callers don't repeat the message themselves"
        )]
        {
            eprintln!("MISE_ENV omits `{}`", language.as_str());
        }
    }
    included
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
    if let Ok(entries) = fs::read_dir(&aube_root) {
        for entry in entries.flatten() {
            let node_modules = entry.path().join("node_modules/.aube/node_modules");
            if node_modules.join(package).is_dir() {
                return Some(node_modules);
            }
        }
    }

    None
}

/// `site-packages` directories of every `pipx:` python package `mise` has
/// installed for the current config.
///
/// Intended for pre-populating an embedded interpreter's `sys.path` so the
/// `et-ws-pyo3-runner` can `import` mise-managed packages without the operator
/// wiring `PYTHONPATH` by hand. Runs `mise ls --current --json` once and, for
/// each `pipx:<pkg>` tool, locates its venv `site-packages` via
/// [`find_site_packages_in`]. Best-effort: returns an empty vec if `mise` is
/// unavailable, exits non-zero, or emits output we can't parse.
#[must_use]
pub fn mise_python_site_packages() -> Vec<PathBuf> {
    if !mise_is_available() {
        return Vec::new();
    }
    let output = std::process::Command::new("mise")
        .args(["ls", "--current", "--json"])
        .output()
        .ok();
    let Some(output) = output.filter(|out| out.status.success()) else {
        return Vec::new();
    };
    let Ok(tools) = serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&output.stdout) else {
        return Vec::new();
    };
    tools
        .iter()
        .filter(|(name, _)| name.starts_with("pipx:"))
        .filter_map(|(_, installs)| {
            // Each tool maps to an array of installs; take the active one (or
            // the first, if none is flagged) and read its `install_path`.
            let array = installs.as_array()?;
            let entry = array
                .iter()
                .find(|entry| entry.get("active").and_then(serde_json::Value::as_bool) == Some(true))
                .or_else(|| array.first())?;
            let path = entry.get("install_path").and_then(serde_json::Value::as_str)?;
            Some(PathBuf::from(path))
        })
        .filter_map(|install| find_site_packages_in(&install))
        .collect()
}

/// Pure-filesystem helper: given a mise `pipx:` `<install>` root, return the
/// venv `site-packages` directory.
///
/// pipx lays each tool out as `<install>/<pkg>/<venv-libdir>/site-packages`,
/// where `<venv-libdir>` is `lib/python<X.Y>` on POSIX and `Lib` (no Python
/// version subdir) on Windows. The `<pkg>` directory name (and the Python
/// version on POSIX) varies, so the variable segments are scanned rather than
/// assumed. Returns the first match, or `None` if nothing under `<install>`
/// has that shape.
#[must_use]
pub fn find_site_packages_in(install: &Path) -> Option<PathBuf> {
    for pkg in fs::read_dir(install).ok()?.flatten() {
        let pkg_path = pkg.path();
        let windows_layout = pkg_path.join("Lib").join("site-packages");
        if windows_layout.is_dir() {
            return Some(windows_layout);
        }
        let Ok(lib_entries) = fs::read_dir(pkg_path.join("lib")) else {
            continue;
        };
        for py in lib_entries.flatten() {
            if !py.file_name().to_string_lossy().starts_with("python") {
                continue;
            }
            let site_packages = py.path().join("site-packages");
            if site_packages.is_dir() {
                return Some(site_packages);
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
