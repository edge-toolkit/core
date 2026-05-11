use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
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
    #[serde(rename = "js-main", default)]
    js_main: Option<String>,
    /// WASI component file (relative to `pkg/`). Set this for modules whose
    /// `main` artifact is a WASI Preview 2 component executed by
    /// `et-ws-wasi-runner` rather than a browser-side JS entry.
    #[serde(rename = "wasi-main", default)]
    wasi_main: Option<String>,
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
    /// WASI component file (relative to `pkg/`). Set this for Rust modules
    /// whose `main` artifact is a WASI Preview 2 component executed by
    /// `et-ws-wasi-runner` rather than a browser-side JS entry built by
    /// wasm-pack.
    #[serde(rename = "wasi-main", default)]
    wasi_main: Option<String>,
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
}

pub fn generate_module_package_json(module_dir: &Path) -> Result<PathBuf> {
    let out_path = module_dir.join("pkg/package.json");
    let package_json = if module_dir.join("pyproject.toml").is_file() {
        package_json_from_pyproject(module_dir)?
    } else if module_dir.join("Cargo.toml").is_file() {
        package_json_from_cargo(module_dir, &out_path)?
    } else {
        return Err(anyhow!(
            "Expected pyproject.toml or Cargo.toml in module directory {:?}",
            module_dir
        ));
    };

    let parent = out_path
        .parent()
        .ok_or_else(|| anyhow!("Output path {:?} has no parent directory", out_path))?;
    fs::create_dir_all(parent).with_context(|| format!("Failed to create output directory: {:?}", parent))?;
    let mut out = serde_json::to_string_pretty(&package_json).context("Failed to serialize package JSON")?;
    out.push('\n');
    fs::write(&out_path, &out).with_context(|| format!("Failed to write {}", out_path.display()))?;

    Ok(out_path)
}

fn package_json_from_pyproject(module_dir: &Path) -> Result<Value> {
    let pyproject_path = module_dir.join("pyproject.toml");
    let pyproject: Pyproject = read_toml(&pyproject_path)?;
    let p = &pyproject.project;
    let ws_module = &pyproject.tool.ws_module;

    // `main` is what every module-fetcher expects to see. Prefer the JS
    // entry (browser modules); fall back to the WASI component (WASI-only
    // modules like wasi-graphics-info).
    let main = ws_module
        .js_main
        .clone()
        .or_else(|| ws_module.wasi_main.clone())
        .ok_or_else(|| anyhow!("[tool.ws-module] in {:?} must set js-main or wasi-main", pyproject_path))?;

    let mut pkg = Map::from_iter([
        ("name".to_string(), json!(p.name)),
        ("type".to_string(), json!("module")),
        ("description".to_string(), json!(p.description.as_deref().unwrap_or(""))),
        ("version".to_string(), json!(p.version)),
        ("license".to_string(), json!(p.license.as_deref().unwrap_or(""))),
        ("main".to_string(), json!(main)),
    ]);
    if let Some(wasi_main) = &ws_module.wasi_main {
        pkg.insert("wasi-main".to_string(), json!(wasi_main));
    }
    if !ws_module.dependencies.is_empty() {
        pkg.insert("dependencies".to_string(), json!(ws_module.dependencies));
    }
    Ok(Value::Object(pkg))
}

fn package_json_from_cargo(module_dir: &Path, out_path: &Path) -> Result<Value> {
    let cargo_toml: CargoPackage = read_toml(&module_dir.join("Cargo.toml"))?;
    let mut pkg = read_package_json(out_path)?.unwrap_or_else(|| {
        let mut pkg = Map::new();
        pkg.insert("name".to_string(), json!(cargo_toml.package.name));
        pkg.insert("type".to_string(), json!("module"));
        pkg
    });

    if !pkg.contains_key("name") {
        pkg.insert("name".to_string(), json!(cargo_toml.package.name));
    }

    let Some(ws_module) = cargo_toml.package.metadata.and_then(|metadata| metadata.ws_module) else {
        return Ok(Value::Object(pkg));
    };

    if let Some(wasi_main) = &ws_module.wasi_main {
        pkg.insert("main".to_string(), json!(wasi_main));
        pkg.insert("wasi-main".to_string(), json!(wasi_main));
    }

    if !ws_module.dependencies.is_empty() {
        let dependencies = pkg
            .entry("dependencies".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let dependency_map = dependencies
            .as_object_mut()
            .ok_or_else(|| anyhow!("{} contains a non-object dependencies field", out_path.display()))?;
        for (name, version) in ws_module.dependencies {
            dependency_map.insert(name, json!(version));
        }
    }

    Ok(Value::Object(pkg))
}

fn read_toml<T>(path: &Path) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let src = fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    toml::from_str(&src).with_context(|| format!("Failed to parse {}", path.display()))
}

fn read_package_json(path: &Path) -> Result<Option<Map<String, Value>>> {
    let src = match fs::read_to_string(path) {
        Ok(src) => src,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("Failed to read {}", path.display())),
    };
    let Value::Object(pkg) =
        serde_json::from_str(&src).with_context(|| format!("Failed to parse {}", path.display()))?
    else {
        return Err(anyhow!("{} must contain a JSON object", path.display()));
    };
    Ok(Some(pkg))
}
