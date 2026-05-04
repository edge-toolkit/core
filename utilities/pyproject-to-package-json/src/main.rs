use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{Map, Value, json};

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
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
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
    let mut pkg = Map::from_iter([
        ("name".to_string(), json!(p.name)),
        ("type".to_string(), json!("module")),
        ("description".to_string(), json!(p.description.as_deref().unwrap_or(""))),
        ("version".to_string(), json!(p.version)),
        ("license".to_string(), json!(p.license.as_deref().unwrap_or(""))),
        ("main".to_string(), json!(pyproject.tool.ws_module.js_main)),
    ]);
    if !pyproject.tool.ws_module.dependencies.is_empty() {
        pkg.insert("dependencies".to_string(), json!(pyproject.tool.ws_module.dependencies));
    }
    let pkg = Value::Object(pkg);

    let out_path = PathBuf::from("pkg/package.json");
    fs::create_dir_all(out_path.parent().unwrap()).unwrap();
    let mut out = serde_json::to_string_pretty(&pkg).unwrap();
    out.push('\n');
    fs::write(&out_path, &out).unwrap_or_else(|e| panic!("Failed to write {}: {e}", out_path.display()));

    println!("Wrote {}", out_path.display());
}
