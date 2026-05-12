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

/// Shared shape of `[tool.ws-module]` (pyproject.toml) and
/// `[package.metadata.ws-module]` (Cargo.toml).
#[derive(Deserialize, Default)]
struct WsModule {
    /// Override for the resolved entry file (relative to `pkg/`). When
    /// `None`, the entry is derived from the package name — see
    /// [`resolve_main`].
    #[serde(default)]
    main: Option<String>,
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct Tool {
    #[serde(rename = "ws-module", default)]
    ws_module: WsModule,
}

#[derive(Deserialize)]
struct Pyproject {
    project: Project,
    #[serde(default)]
    tool: Option<Tool>,
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
    ws_module: Option<WsModule>,
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
    let ws_module = pyproject.tool.map(|t| t.ws_module).unwrap_or_default();

    let pkg_dir = module_dir.join("pkg");
    let kind = detect_python_kind(module_dir);
    let main = resolve_main(&pkg_dir, &p.name, kind, ws_module.main.as_deref())?;

    let mut pkg = Map::from_iter([
        ("name".to_string(), json!(p.name)),
        ("type".to_string(), json!("module")),
        ("description".to_string(), json!(p.description.as_deref().unwrap_or(""))),
        ("version".to_string(), json!(p.version)),
        ("license".to_string(), json!(p.license.as_deref().unwrap_or(""))),
        ("main".to_string(), json!(main)),
    ]);
    if !ws_module.dependencies.is_empty() {
        pkg.insert("dependencies".to_string(), json!(ws_module.dependencies));
    }
    Ok(Value::Object(pkg))
}

fn package_json_from_cargo(module_dir: &Path, out_path: &Path) -> Result<Value> {
    let cargo_toml_path = module_dir.join("Cargo.toml");
    let cargo_toml_src = fs::read_to_string(&cargo_toml_path)
        .with_context(|| format!("Failed to read {}", cargo_toml_path.display()))?;
    let cargo_toml: CargoPackage =
        toml::from_str(&cargo_toml_src).with_context(|| format!("Failed to parse {}", cargo_toml_path.display()))?;
    let crate_name = cargo_toml.package.name;
    let kind = detect_cargo_kind(&cargo_toml_src);
    let mut pkg = read_package_json(out_path)?.unwrap_or_else(|| {
        let mut pkg = Map::new();
        pkg.insert("name".to_string(), json!(crate_name));
        pkg.insert("type".to_string(), json!("module"));
        pkg
    });

    if !pkg.contains_key("name") {
        pkg.insert("name".to_string(), json!(crate_name));
    }

    let ws_module = cargo_toml
        .package
        .metadata
        .and_then(|metadata| metadata.ws_module)
        .unwrap_or_default();

    let existing_main = pkg.get("main").and_then(|v| v.as_str()).map(str::to_string);
    let main_override = ws_module.main.as_deref().or(existing_main.as_deref());
    let pkg_dir = module_dir.join("pkg");
    let main = resolve_main(&pkg_dir, &crate_name, kind, main_override)?;
    pkg.insert("main".to_string(), json!(main));

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

/// Whether a module is built as a WASI Preview 2 component or as JS that
/// browser/Pyodide loads.
#[derive(Clone, Copy)]
enum ModuleKind {
    Wasi,
    Js,
}

impl ModuleKind {
    fn extension(self) -> &'static str {
        match self {
            ModuleKind::Wasi => "wasm",
            ModuleKind::Js => "js",
        }
    }
}

/// `componentize-py bindings` writes a `wit_world/` package next to the
/// module's `pyproject.toml`. Its presence is what tells us this is a
/// WASI Python module rather than a Pyodide module.
fn detect_python_kind(module_dir: &Path) -> ModuleKind {
    if module_dir.join("wit_world").is_dir() {
        ModuleKind::Wasi
    } else {
        ModuleKind::Js
    }
}

/// WASI Rust modules use `wit-bindgen` to generate component bindings;
/// wasm-pack browser modules don't. The substring check covers
/// `[dependencies]`, `[target.*.dependencies]`, and workspace-dep lines
/// alike without needing to model Cargo.toml's full dependency tree.
fn detect_cargo_kind(cargo_toml_src: &str) -> ModuleKind {
    if cargo_toml_src.contains("wit-bindgen") {
        ModuleKind::Wasi
    } else {
        ModuleKind::Js
    }
}

/// Resolve the `main` entry file in `pkg_dir`.
///
/// If `main_override` is set, that filename is used. Otherwise the entry
/// is derived from `name` by trying both its `_` and `-` variants with the
/// extension dictated by `kind` (`.wasm` for WASI, `.js` for browser/Pyodide).
/// The resolved file must exist in `pkg_dir`; this errors otherwise.
fn resolve_main(pkg_dir: &Path, name: &str, kind: ModuleKind, main_override: Option<&str>) -> Result<String> {
    if let Some(main) = main_override {
        if !pkg_dir.join(main).is_file() {
            return Err(anyhow!("main = {:?} does not exist in {}", main, pkg_dir.display()));
        }
        return Ok(main.to_string());
    }
    let underscored = name.replace('-', "_");
    let hyphenated = name.replace('_', "-");
    let stems: &[&str] = if underscored == hyphenated {
        &[underscored.as_str()]
    } else {
        &[underscored.as_str(), hyphenated.as_str()]
    };
    let ext = kind.extension();
    for stem in stems {
        let candidate = format!("{stem}.{ext}");
        if pkg_dir.join(&candidate).is_file() {
            return Ok(candidate);
        }
    }
    Err(anyhow!(
        "No main file in {}; expected {underscored}.{ext} or {hyphenated}.{ext} (override with [ws-module] main)",
        pkg_dir.display()
    ))
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
