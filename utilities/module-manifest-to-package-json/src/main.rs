use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

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

#[derive(Deserialize)]
struct CargoPackage {
    package: CargoPackageMetadata,
}

#[derive(Deserialize)]
struct CargoPackageMetadata {
    name: String,
    metadata: Option<CargoMetadata>,
}

#[derive(Deserialize)]
struct CargoMetadata {
    #[serde(rename = "ws-module")]
    ws_module: Option<CargoWsModule>,
}

#[derive(Deserialize)]
struct CargoWsModule {
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
}

fn main() {
    let out_path = PathBuf::from("pkg/package.json");
    let package_json = if Path::new("pyproject.toml").is_file() {
        package_json_from_pyproject()
    } else if Path::new("Cargo.toml").is_file() {
        package_json_from_cargo(&out_path)
    } else {
        panic!("Expected pyproject.toml or Cargo.toml in the current directory");
    };

    fs::create_dir_all(out_path.parent().unwrap()).unwrap();
    let mut out = serde_json::to_string_pretty(&package_json).unwrap();
    out.push('\n');
    fs::write(&out_path, &out).unwrap_or_else(|e| panic!("Failed to write {}: {e}", out_path.display()));

    println!("Wrote {}", out_path.display());
}

fn package_json_from_pyproject() -> Value {
    let pyproject_path = PathBuf::from("pyproject.toml");
    let pyproject: Pyproject = read_toml(&pyproject_path);
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
    Value::Object(pkg)
}

fn package_json_from_cargo(out_path: &Path) -> Value {
    let cargo_toml: CargoPackage = read_toml(Path::new("Cargo.toml"));
    let mut pkg = read_package_json(out_path).unwrap_or_else(|| {
        let mut pkg = Map::new();
        pkg.insert("name".to_string(), json!(cargo_toml.package.name));
        pkg.insert("type".to_string(), json!("module"));
        pkg
    });

    if !pkg.contains_key("name") {
        pkg.insert("name".to_string(), json!(cargo_toml.package.name));
    }

    let Some(ws_module) = cargo_toml.package.metadata.and_then(|metadata| metadata.ws_module) else {
        return Value::Object(pkg);
    };

    if !ws_module.dependencies.is_empty() {
        let dependencies = pkg
            .entry("dependencies".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let dependency_map = dependencies
            .as_object_mut()
            .unwrap_or_else(|| panic!("{} contains a non-object dependencies field", out_path.display()));
        for (name, version) in ws_module.dependencies {
            dependency_map.insert(name, json!(version));
        }
    }

    Value::Object(pkg)
}

fn read_toml<T>(path: &Path) -> T
where
    T: for<'de> Deserialize<'de>,
{
    let src = fs::read_to_string(path).unwrap_or_else(|e| panic!("Failed to read {}: {e}", path.display()));
    toml::from_str(&src).unwrap_or_else(|e| panic!("Failed to parse {}: {e}", path.display()))
}

fn read_package_json(path: &Path) -> Option<Map<String, Value>> {
    let src = fs::read_to_string(path).ok()?;
    let Value::Object(pkg) =
        serde_json::from_str(&src).unwrap_or_else(|e| panic!("Failed to parse {}: {e}", path.display()))
    else {
        panic!("{} must contain a JSON object", path.display());
    };
    Some(pkg)
}
