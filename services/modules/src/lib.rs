use std::path::PathBuf;

use actix_files::Files;
use actix_web::web;
use edge_toolkit::config::default_modules_folders;
use fs_err as fs;
use serde::Deserialize;
use serde_default::DefaultFromSerde;
use serde_inline_default::serde_inline_default;

pub mod routes;

pub use self::routes::list_modules_handler;

/// Modules config.
#[serde_inline_default]
#[derive(Clone, Debug, DefaultFromSerde, Deserialize)]
#[non_exhaustive]
pub struct ModulesConfig {
    #[serde(default = "default_modules_folders")]
    pub paths: Vec<PathBuf>,
    /// Name of the module served at `/`, exactly as its `package.json` declares it.
    ///
    /// No default, because which module is a deployment's front page is a property of that deployment and
    /// not of this server: a name defaulted here would be one project's, and every other one would be
    /// carrying it around as dead configuration. Unset serves nothing at `/` and is not an error -- a
    /// deployment whose agents are headless runners has no page to put there, and demanding one would make
    /// every such deployment name a module it never loads.
    #[serde(default)]
    pub root: String,
    /// Also serve whatever the mise config in scope staged, on top of `paths`.
    ///
    /// A deployment that installs its modules as `[tools]` has already said which ones it serves. Repeating
    /// that as a list of directories would be the same set written twice, in a form nothing can write down --
    /// where mise puts a package is decided per backend and platform when it installs.
    ///
    /// Defaults to whether mise is there to ask, because mise being on `PATH` is exactly what makes a staged
    /// module findable: a deployment that provisioned its modules some other way has no mise to consult and
    /// gets nothing extra, and one that did needs to declare nothing. Set it to `false` to serve only
    /// `paths` on a host that does have mise -- a config whose tool set mixes modules with development
    /// tooling wants that, since discovery cannot tell one from the other.
    #[serde(default = "edge_toolkit::config::mise_is_available")]
    pub mise_discover: bool,
}

impl ModulesConfig {
    /// Config that serves exactly `paths`, which is what a caller naming directories outright wants.
    #[must_use]
    pub const fn new(paths: Vec<PathBuf>, root: String) -> Self {
        Self {
            paths,
            root,
            mise_discover: false,
        }
    }
}

/// The package name a `package.json` declares, exactly as written.
fn read_package_name(package_json: &std::path::Path) -> Option<String> {
    let content = fs::read_to_string(package_json).ok()?;
    let value: serde_json::Value = serde_json::from_str(&content).ok()?;
    Some(value.get("name")?.as_str()?.to_string())
}

/// Scan all configured module paths and return a sorted list of `(name, pkg_dir)` pairs.
#[must_use]
pub fn list_modules(config: &ModulesConfig) -> Vec<(String, PathBuf)> {
    let mut modules: Vec<(String, PathBuf)> = Vec::new();
    let mut paths = config.paths.clone();
    if config.mise_discover {
        paths.extend(edge_toolkit::config::mise_staged_module_dirs());
        // A directory reached both ways would otherwise be served under two routes, which is a startup
        // error rather than a duplicate listing.
        paths.sort();
        paths.dedup();
    }
    for path in &paths {
        let pkg_dir = path.join("pkg");
        if pkg_dir.is_dir() {
            let name = read_package_name(&pkg_dir.join("package.json"))
                .or_else(|| path.file_name().and_then(|name| name.to_str()).map(str::to_string));
            if let Some(name) = name {
                modules.push((name, pkg_dir));
            }
        } else if path.join("package.json").is_file() {
            let name = read_package_name(&path.join("package.json"))
                .or_else(|| path.file_name().and_then(|name| name.to_str()).map(str::to_string));
            if let Some(name) = name {
                modules.push((name, path.clone()));
            }
        } else if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                // `Path::is_dir` follows symlinks; `entry.file_type().is_dir()` would skip them. mise's aube npm
                // backend lays out `node_modules/.aube/node_modules/<pkg>` as a symlink farm, so the symlink-following
                // variant is required to discover those packages.
                let entry_path = entry.path();
                if entry_path.is_dir() && !paths.contains(&entry_path) {
                    let pkg_dir = entry_path.join("pkg");
                    if pkg_dir.is_dir() {
                        let name = read_package_name(&pkg_dir.join("package.json"))
                            .or_else(|| entry.file_name().to_str().map(str::to_string));
                        if let Some(name) = name {
                            modules.push((name, pkg_dir));
                        }
                    } else if entry_path.join("package.json").is_file() {
                        let name = read_package_name(&entry_path.join("package.json"))
                            .or_else(|| entry.file_name().to_str().map(str::to_string));
                        if let Some(name) = name {
                            modules.push((name, entry_path));
                        }
                    } else {
                        // No `pkg/` and no root `package.json`; not a module dir.
                    }
                }
            }
        } else {
            // Configured path is neither a module dir nor a readable parent dir; skip silently.
        }
    }
    modules.sort_by(|lhs, rhs| lhs.0.cmp(&rhs.0));
    modules
}

/// Register `GET /modules/` (JSON list), `GET /modules/{name}/...` (static files), and `GET /` (root module).
///
/// An unset `config.root` serves nothing at `/`, leaving the module routes above as the whole surface.
///
/// # Panics
/// Panics if `config.root` names a module none of the scanned `config.paths` provides -- naming a front page
/// that isn't there is a config error, and it is fatal early so the operator sees it at startup rather than
/// as a 404 much later.
#[expect(
    clippy::panic,
    reason = "a root module named but absent is a config error; failing fast at startup is intentional"
)]
pub fn configure(cfg: &mut web::ServiceConfig, config: &ModulesConfig) {
    let modules = list_modules(config);

    let _routed = cfg.route("/modules/", web::get().to(list_modules_handler));
    for (name, pkg_dir) in &modules {
        let _served = cfg.service(Files::new(&format!("/modules/{name}"), pkg_dir));
    }

    if config.root.is_empty() {
        return;
    }
    let root_module_dir = modules.iter().find(|(name, _)| name == &config.root).map_or_else(
        || {
            let served: Vec<&str> = modules.iter().map(|(name, _)| name.as_str()).collect();
            panic!("Root module '{}' not found; serving {served:?}", config.root)
        },
        |(_, path)| path.clone(),
    );
    // Registered last: `Files::new("/")` matches everything, so anything mounted after it is unreachable.
    let _root_served = cfg.service(
        Files::new("/", root_module_dir)
            .index_file("index.html")
            .prefer_utf8(true),
    );
}
