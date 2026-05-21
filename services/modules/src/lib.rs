use std::path::PathBuf;

use actix_files::Files;
use actix_web::{HttpResponse, web};
use edge_toolkit::config::default_modules_folders;
use serde::Deserialize;
use serde_default::DefaultFromSerde;
use serde_inline_default::serde_inline_default;

/// Modules config.
#[serde_inline_default]
#[derive(Clone, Debug, DefaultFromSerde, Deserialize)]
#[non_exhaustive]
pub struct ModulesConfig {
    #[serde(default = "default_modules_folders")]
    pub paths: Vec<PathBuf>,
    #[serde_inline_default(String::from("et-ws-server-static"))]
    pub root: String,
}

impl ModulesConfig {
    #[must_use]
    pub const fn new(paths: Vec<PathBuf>, root: String) -> Self {
        Self { paths, root }
    }
}

fn read_package_name(package_json: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(package_json).ok()?;
    let value: serde_json::Value = serde_json::from_str(&content).ok()?;
    value.get("name")?.as_str().map(str::to_string)
}

/// Scan all configured module paths and return a sorted list of `(name, pkg_dir)` pairs.
#[must_use]
pub fn list_modules(config: &ModulesConfig) -> Vec<(String, PathBuf)> {
    let mut modules: Vec<(String, PathBuf)> = Vec::new();
    for path in &config.paths {
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
        } else if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                // `Path::is_dir` follows symlinks; `entry.file_type().is_dir()`
                // would skip them. mise's aube npm backend lays out
                // `node_modules/.aube/node_modules/<pkg>` as a symlink farm,
                // so the symlink-following variant is required to discover
                // those packages.
                let entry_path = entry.path();
                if entry_path.is_dir() && !config.paths.contains(&entry_path) {
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

#[expect(
    clippy::single_call_fn,
    reason = "actix-web route handler; registered via web::get().to(...)"
)]
async fn list_modules_handler(config: web::Data<ModulesConfig>) -> HttpResponse {
    let names: Vec<String> = list_modules(&config).into_iter().map(|(name, _)| name).collect();
    HttpResponse::Ok().json(names)
}

/// Register `GET /modules/` (JSON list), `GET /modules/{name}/...` (static files),
/// and `GET /` (root module).
///
/// # Panics
/// Panics if `config.root` is not present in `config.paths` — server config
/// is fatal early so the operator sees the misconfiguration at startup.
#[expect(
    clippy::panic,
    reason = "missing root module is a config error; failing fast at startup is intentional"
)]
pub fn configure(cfg: &mut web::ServiceConfig, config: &ModulesConfig) {
    let modules = list_modules(config);

    let root_module_dir = modules.iter().find(|(name, _)| name == &config.root).map_or_else(
        || panic!("Root module '{}' not found", config.root),
        |(_, path)| path.clone(),
    );

    let _routed = cfg.route("/modules/", web::get().to(list_modules_handler));
    for (name, pkg_dir) in &modules {
        let _served = cfg.service(Files::new(&format!("/modules/{name}"), pkg_dir));
    }
    let _root_served = cfg.service(
        Files::new("/", root_module_dir)
            .index_file("index.html")
            .prefer_utf8(true),
    );
}
