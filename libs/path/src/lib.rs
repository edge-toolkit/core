//! Path utilities shared across the workspace.
//!
//! Root-finding has two entry points: [`find_project_root`] (from an explicit
//! start, used by the runtime `edge_toolkit::config::get_project_root`) and
//! [`find_project_root_from_manifest`] (from `CARGO_MANIFEST_DIR`, used by build
//! scripts). The path builders [`absolute_from`] and [`relative_path_from`]
//! render POSIX-style paths for `mise.toml` / `docker-compose.yaml` generation.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

/// Marker file that identifies the repository root. `.dprint.jsonc` is present
/// at the root (and nowhere above it), so it is a reliable anchor.
const ROOT_MARKER: &str = ".dprint.jsonc";

/// Walk up from `start` to the first ancestor that contains `.dprint.jsonc`.
///
/// Falls back to `start` itself when no ancestor has the marker, mirroring the
/// runtime helper's "use what we have" behaviour.
#[must_use]
pub fn find_project_root(start: &Path) -> PathBuf {
    start
        .ancestors()
        .find(|dir| dir.join(ROOT_MARKER).is_file())
        .map_or_else(|| start.to_path_buf(), Path::to_path_buf)
}

/// Locate the repository root from a build script.
///
/// Reads `CARGO_MANIFEST_DIR` (which cargo sets for build scripts), so this is
/// the single sanctioned use of that variable (see the `no-cargo-manifest-dir`
/// ast-grep rule). Outside a build script the variable is unset and this
/// returns a meaningless path; use [`find_project_root`] with an explicit
/// start, or `edge_toolkit::config::get_project_root`, instead.
#[must_use]
pub fn find_project_root_from_manifest() -> PathBuf {
    const CARGO_MANIFEST_DIR: &str = "CARGO_MANIFEST_DIR";
    let manifest = std::env::var(CARGO_MANIFEST_DIR).unwrap_or_default();
    find_project_root(Path::new(&manifest))
}

/// Resolve `path` against `base` when relative, then lexically normalize it.
#[must_use]
pub fn absolute_from(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        normalize_path(path)
    } else {
        normalize_path(&base.join(path))
    }
}

/// Build a relative path from `from_dir` to `target`, always joined with `/`.
///
/// The result is rendered as a POSIX string regardless of host OS, because
/// every caller writes it into generated `mise.toml` / `docker-compose.yaml`
/// output -- both of which expect forward-slash separators even on Windows.
#[must_use]
pub fn relative_path_from(from_dir: &Path, target: &Path) -> String {
    let from_components = normal_components(&normalize_path(from_dir));
    let target_components = normal_components(&normalize_path(target));
    let common_len = from_components
        .iter()
        .zip(target_components.iter())
        .take_while(|(from, target)| from == target)
        .count();

    let mut parts: Vec<String> = Vec::new();
    for _ in common_len..from_components.len() {
        parts.push("..".to_string());
    }
    for component in target_components.iter().skip(common_len) {
        parts.push(component.to_string_lossy().into_owned());
    }

    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

fn normal_components(path: &Path) -> Vec<OsString> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_os_string()),
            Component::Prefix(_) | Component::RootDir | Component::CurDir | Component::ParentDir => None,
        })
        .collect()
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                let _popped = normalized.pop();
            }
            Component::Normal(value) => normalized.push(value),
        }
    }
    normalized
}
