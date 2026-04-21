use std::fs;
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
struct Project {
    name: String,
    version: String,
    description: Option<String>,
    license: Option<String>,
}

#[derive(Deserialize)]
struct WsModule {
    #[serde(rename = "js-main")]
    js_main: String,
}

#[derive(Deserialize)]
struct Tool {
    #[serde(rename = "ws-module")]
    ws_module: WsModule,
}

#[derive(Deserialize)]
struct Pyproject {
    project: Project,
    tool: Tool,
}

fn main() {
    let pyproject_path = PathBuf::from("pyproject.toml");
    let src = fs::read_to_string(&pyproject_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {e}", pyproject_path.display()));

    let pyproject: Pyproject = toml::from_str(&src).unwrap_or_else(|e| panic!("Failed to parse pyproject.toml: {e}"));

    let p = &pyproject.project;
    let pkg: Value = json!({
        "name": p.name,
        "type": "module",
        "description": p.description.as_deref().unwrap_or(""),
        "version": p.version,
        "license": p.license.as_deref().unwrap_or(""),
        "main": pyproject.tool.ws_module.js_main,
    });

    let out_path = PathBuf::from("pkg/package.json");
    fs::create_dir_all(out_path.parent().unwrap()).unwrap();
    let mut out = serde_json::to_string_pretty(&pkg).unwrap();
    out.push('\n');
    fs::write(&out_path, &out).unwrap_or_else(|e| panic!("Failed to write {}: {e}", out_path.display()));

    println!("Wrote {}", out_path.display());
}
