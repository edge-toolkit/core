#![expect(
    unused_results,
    reason = "serde_json::Map::insert discards the prior Value at each key, which is the intended overwrite"
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use fs_err as fs;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::error::{CliError, parse_json, parse_toml, serialize_json_pretty};

#[derive(Deserialize)]
struct Project {
    name: String,
    version: String,
    description: Option<String>,
    license: Option<String>,
    #[serde(default)]
    urls: BTreeMap<String, String>,
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
struct CargoToml {
    package: Option<CargoPackageMetadata>,
    workspace: Option<CargoWorkspace>,
}

#[derive(Deserialize)]
struct CargoPackageMetadata {
    name: String,
    version: Option<MaybeInherited>,
    repository: Option<MaybeInherited>,
    metadata: Option<CargoMetadata>,
}

#[derive(Deserialize)]
struct CargoMetadata {
    #[serde(rename = "ws-module")]
    ws_module: Option<WsModule>,
}

#[derive(Deserialize)]
struct CargoWorkspace {
    package: Option<WorkspacePackage>,
}

#[derive(Deserialize, Default)]
struct WorkspacePackage {
    version: Option<String>,
    repository: Option<String>,
}

/// A Cargo `[package]` field that can be either a literal value
/// (`version = "0.1.0"`) or inherited from the workspace
/// (`version.workspace = true`).
#[derive(Deserialize)]
#[serde(untagged)]
enum MaybeInherited {
    Direct(String),
    Workspace {
        #[expect(
            dead_code,
            reason = "field exists only to make serde's untagged deserializer pick this variant"
        )]
        workspace: bool,
    },
}

pub fn generate_module_package_json(module_dir: &Path) -> Result<PathBuf, CliError> {
    let out_path = module_dir.join("pkg/package.json");
    let package_json = if module_dir.join("pyproject.toml").is_file() {
        package_json_from_pyproject(module_dir)?
    } else if module_dir.join("Cargo.toml").is_file() {
        package_json_from_cargo(module_dir, &out_path)?
    } else {
        return Err(CliError::MissingManifest(module_dir.to_path_buf()));
    };

    let parent = out_path
        .parent()
        .ok_or_else(|| CliError::NoParentDir(out_path.clone()))?;
    fs::create_dir_all(parent)?;
    let mut out = serialize_json_pretty(&package_json)?;
    out.push('\n');
    fs::write(&out_path, &out)?;

    Ok(out_path)
}

fn package_json_from_pyproject(module_dir: &Path) -> Result<Value, CliError> {
    let pyproject_path = module_dir.join("pyproject.toml");
    let pyproject: Pyproject = read_toml(&pyproject_path)?;
    let project = &pyproject.project;
    let ws_module = pyproject.tool.map(|tool| tool.ws_module).unwrap_or_default();

    let pkg_dir = module_dir.join("pkg");
    let kind = detect_python_kind(module_dir);
    let main = resolve_main(&pkg_dir, &project.name, kind, ws_module.main.as_deref())?;

    let mut pkg = Map::from_iter([
        ("name".to_string(), json!(project.name)),
        ("type".to_string(), json!("module")),
        (
            "description".to_string(),
            json!(project.description.as_deref().unwrap_or("")),
        ),
        ("version".to_string(), json!(project.version)),
        ("license".to_string(), json!(project.license.as_deref().unwrap_or(""))),
        ("main".to_string(), json!(main)),
    ]);
    if let Some(repo) = project_repository(&project.urls) {
        pkg.insert("repository".to_string(), repository_json(repo));
    }
    if !ws_module.dependencies.is_empty() {
        pkg.insert("dependencies".to_string(), json!(ws_module.dependencies));
    }
    Ok(Value::Object(pkg))
}

/// PEP 621 `[project.urls]` is a free-form map keyed by display name. The
/// PyPI-recommended convention is to call the source-of-truth URL one of
/// these (case-sensitive); we accept all of them.
fn project_repository(urls: &BTreeMap<String, String>) -> Option<&str> {
    ["Repository", "repository", "Source", "source"]
        .iter()
        .find_map(|key| urls.get(*key))
        .map(String::as_str)
}

fn package_json_from_cargo(module_dir: &Path, out_path: &Path) -> Result<Value, CliError> {
    let cargo_toml_path = module_dir.join("Cargo.toml");
    let cargo_toml_src = fs::read_to_string(&cargo_toml_path)?;
    let cargo_toml: CargoToml = parse_toml(&cargo_toml_path, &cargo_toml_src)?;
    let package = cargo_toml
        .package
        .ok_or_else(|| CliError::MissingPackageSection(cargo_toml_path.clone()))?;
    let crate_name = package.name;
    let kind = detect_cargo_kind(&cargo_toml_src);
    let workspace = find_workspace_package(module_dir)?;

    let mut pkg = read_package_json(out_path)?.unwrap_or_else(|| {
        let mut pkg = Map::new();
        pkg.insert("name".to_string(), json!(crate_name));
        pkg.insert("type".to_string(), json!("module"));
        pkg
    });

    if !pkg.contains_key("name") {
        pkg.insert("name".to_string(), json!(crate_name));
    }
    let ws_version = workspace.as_ref().and_then(|ws| ws.version.as_deref());
    let ws_repository = workspace.as_ref().and_then(|ws| ws.repository.as_deref());
    if !pkg.contains_key("version")
        && let Some(version) = resolve_inherited(package.version.as_ref(), ws_version)
    {
        pkg.insert("version".to_string(), json!(version));
    }
    if !pkg.contains_key("repository")
        && let Some(repo) = resolve_inherited(package.repository.as_ref(), ws_repository)
    {
        pkg.insert("repository".to_string(), repository_json(&repo));
    }

    let ws_module = package
        .metadata
        .and_then(|metadata| metadata.ws_module)
        .unwrap_or_default();

    let existing_main = pkg.get("main").and_then(|value| value.as_str()).map(str::to_string);
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
            .ok_or_else(|| CliError::NonObjectDependencies(out_path.to_path_buf()))?;
        for (name, version) in ws_module.dependencies {
            dependency_map.insert(name, json!(version));
        }
    }

    Ok(Value::Object(pkg))
}

/// Resolve a `[package]` field that may be inherited from the workspace.
/// `direct` is the value read from the crate's Cargo.toml; `workspace`
/// is the corresponding `[workspace.package]` value (if any). Returns
/// the literal direct value, or the workspace value when the crate
/// declares `field.workspace = true`.
fn resolve_inherited(direct: Option<&MaybeInherited>, workspace: Option<&str>) -> Option<String> {
    match direct {
        Some(MaybeInherited::Direct(value)) => Some(value.clone()),
        Some(MaybeInherited::Workspace { .. }) => workspace.map(str::to_string),
        None => None,
    }
}

/// Walk parents of `start` looking for a Cargo.toml containing a
/// `[workspace]` table; return its `[workspace.package]` if present.
fn find_workspace_package(start: &Path) -> Result<Option<WorkspacePackage>, CliError> {
    for dir in start.ancestors().skip(1) {
        let cargo = dir.join("Cargo.toml");
        if !cargo.is_file() {
            continue;
        }
        let toml: CargoToml = read_toml(&cargo)?;
        if let Some(ws) = toml.workspace {
            return Ok(ws.package);
        }
    }
    Ok(None)
}

/// npm's `repository` field accepts either a bare URL string or the
/// object form. The object form matches what wasm-pack emits, so we use
/// that for visual consistency across generated package.json files.
fn repository_json(url: &str) -> Value {
    json!({ "type": "git", "url": url })
}

/// Whether a module is built as a WASI Preview 2 component or as JS that
/// browser/Pyodide loads.
#[derive(Clone, Copy)]
enum ModuleKind {
    Wasi,
    Js,
}

impl ModuleKind {
    const fn extension(self) -> &'static str {
        match self {
            Self::Wasi => "wasm",
            Self::Js => "js",
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
fn resolve_main(pkg_dir: &Path, name: &str, kind: ModuleKind, main_override: Option<&str>) -> Result<String, CliError> {
    if let Some(main) = main_override {
        if !pkg_dir.join(main).is_file() {
            return Err(CliError::MissingMainFile {
                main: main.to_string(),
                dir: pkg_dir.to_path_buf(),
            });
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
    Err(CliError::UnresolvedMainFile {
        dir: pkg_dir.to_path_buf(),
        underscored,
        hyphenated,
        ext,
    })
}

fn read_toml<T>(path: &Path) -> Result<T, CliError>
where
    T: for<'de> Deserialize<'de>,
{
    let src = fs::read_to_string(path)?;
    parse_toml(path, &src)
}

fn read_package_json(path: &Path) -> Result<Option<Map<String, Value>>, CliError> {
    let src = match fs::read_to_string(path) {
        Ok(src) => src,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(CliError::Io(source)),
    };
    let Value::Object(pkg) = parse_json(path, &src)? else {
        return Err(CliError::NonObjectPackageJson(path.to_path_buf()));
    };
    Ok(Some(pkg))
}
