use std::path::PathBuf;

use actix_files::Files;
use actix_web::{HttpResponse, web};
use edge_toolkit::ws_server::{Config, ModulesConfig};

fn read_package_name(package_json: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(package_json).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    v.get("name")?.as_str().map(str::to_string)
}

/// Scan all configured module paths and return a sorted list of (name, pkg_dir) pairs.
pub fn list_modules(config: &ModulesConfig) -> Vec<(String, PathBuf)> {
    let mut modules: Vec<(String, PathBuf)> = Vec::new();
    for path in &config.paths {
        let pkg_dir = path.join("pkg");
        if pkg_dir.is_dir() {
            let name = read_package_name(&pkg_dir.join("package.json"))
                .or_else(|| path.file_name().and_then(|n| n.to_str()).map(str::to_string));
            if let Some(name) = name {
                modules.push((name, pkg_dir));
            }
        } else if path.join("package.json").is_file() {
            let name = read_package_name(&path.join("package.json"))
                .or_else(|| path.file_name().and_then(|n| n.to_str()).map(str::to_string));
            if let Some(name) = name {
                modules.push((name, path.clone()));
            }
        } else if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type()
                    && file_type.is_dir()
                    && !config.paths.contains(&entry.path())
                {
                    let entry_path = entry.path();
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
                    }
                }
            }
        }
    }
    modules.sort_by(|a, b| a.0.cmp(&b.0));
    modules
}

async fn list_modules_handler(config: web::Data<Config>) -> HttpResponse {
    let names: Vec<String> = list_modules(&config.modules)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    HttpResponse::Ok().json(names)
}

/// Register `GET /modules/` (JSON list), `GET /modules/{name}/...` (static files),
/// and `GET /` (root module).
pub fn configure(cfg: &mut web::ServiceConfig, config: &Config) {
    let modules = list_modules(&config.modules);

    let root_module_dir = modules
        .iter()
        .find(|(name, _)| name == &config.modules.root)
        .map(|(_, path)| path.clone())
        .unwrap_or_else(|| panic!("Root module '{}' not found", config.modules.root));

    cfg.route("/modules/", web::get().to(list_modules_handler));
    for (name, pkg_dir) in &modules {
        cfg.service(Files::new(&format!("/modules/{name}"), pkg_dir));
    }
    cfg.service(
        Files::new("/", root_module_dir)
            .index_file("index.html")
            .prefer_utf8(true),
    );
}
