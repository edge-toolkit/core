#![expect(
    clippy::single_call_fn,
    reason = "et-cli decomposes scenario generation into named pipeline stages; each invoked once for readability"
)]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use clap::ValueEnum;
use edge_toolkit::input::ClusterInput;
use fs_err as fs;
use serde::Deserialize;

mod deployment_types;
mod error;
mod module_package_json;

pub use self::deployment_types::{
    docker_image_module_paths, generate_docker_compose_deployment, generate_mise_deployment, scenario_module_paths,
};
pub use self::error::CliError;
pub use self::module_package_json::generate_module_package_json;

#[expect(
    clippy::exhaustive_enums,
    reason = "OutputType enumerates the supported deployment formats; downstream code matches exhaustively"
)]
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum OutputType {
    #[default]
    Mise,
    #[serde(rename = "docker-compose", alias = "docker_compose")]
    DockerCompose,
}

impl OutputType {
    pub const ALL: &'static [Self] = &[Self::Mise, Self::DockerCompose];

    #[must_use]
    pub const fn output_file_name(self) -> &'static str {
        match self {
            Self::Mise => "mise.toml",
            Self::DockerCompose => "compose.yaml",
        }
    }
}

fn generated_output_files(output_types: &[OutputType]) -> Vec<&'static str> {
    let mut files = Vec::new();
    for output_type in output_types {
        files.push(output_type.output_file_name());
    }
    files
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DeploymentSummary {
    pub cluster_name: String,
    pub agent_templates: usize,
    pub module_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RegeneratedScenario {
    pub input_file: PathBuf,
    pub output_dir: PathBuf,
    pub summary: DeploymentSummary,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[non_exhaustive]
pub struct PackageJson {
    pub name: Option<String>,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
struct PyprojectPackage {
    project: Option<PyprojectProject>,
    tool: Option<PyprojectTool>,
}

#[derive(Debug, Default, Deserialize)]
struct PyprojectProject {
    name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct PyprojectTool {
    #[serde(rename = "ws-module")]
    ws_module: Option<PyprojectWsModule>,
}

#[derive(Debug, Default, Deserialize)]
struct PyprojectWsModule {
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
struct CargoPackage {
    package: Option<CargoPackageMetadata>,
}

#[derive(Debug, Default, Deserialize)]
struct CargoPackageMetadata {
    name: Option<String>,
    metadata: Option<CargoMetadata>,
}

#[derive(Debug, Default, Deserialize)]
struct CargoMetadata {
    #[serde(rename = "ws-module")]
    ws_module: Option<CargoWsModule>,
}

#[derive(Debug, Default, Deserialize)]
struct CargoWsModule {
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ModuleRegistryEntry {
    pub mise_path: String,
    pub docker_path: String,
    pub dependencies: BTreeSet<String>,
}

pub fn generate_deployment(
    input_file: &Path,
    output_dir: &Path,
    output_type: Option<OutputType>,
) -> Result<DeploymentSummary, CliError> {
    let cluster = load_cluster_input(input_file)?;
    let output_type = output_type
        .map(Ok)
        .or_else(|| cluster.deployment_type.as_deref().map(output_type_from_input))
        .unwrap_or(Ok(OutputType::Mise))?;

    let module_names = cluster_module_names(&cluster);
    generate_deployment_outputs(&cluster, output_dir, &[output_type])?;

    Ok(deployment_summary(
        cluster.cluster_name,
        cluster.agents.len(),
        module_names,
    ))
}

pub fn load_cluster_input(input_file: &Path) -> Result<ClusterInput, CliError> {
    let content = fs::read_to_string(input_file)?;

    Ok(serde_yaml::from_str(&content)?)
}

pub fn regenerate_verification(
    verification_root: &Path,
    output_type: Option<OutputType>,
) -> Result<Vec<RegeneratedScenario>, CliError> {
    let scenarios = discover_verification_scenarios(verification_root)?;

    let mut regenerated = Vec::with_capacity(scenarios.len());
    let mut seen_output_dirs = BTreeSet::new();
    for (input_file, output_dir) in scenarios {
        if !seen_output_dirs.insert(output_dir.clone()) {
            return Err(CliError::DuplicateScenarioOutput {
                root: verification_root.to_path_buf(),
                output: output_dir,
            });
        }

        let cluster = load_cluster_input(&input_file)?;
        let module_names = cluster_module_names(&cluster);
        let output_types = output_type.as_ref().map_or(OutputType::ALL, std::slice::from_ref);

        generate_deployment_outputs(&cluster, &output_dir, output_types)?;
        let summary = deployment_summary(cluster.cluster_name, cluster.agents.len(), module_names);
        regenerated.push(RegeneratedScenario {
            input_file,
            output_dir,
            summary,
        });
    }

    Ok(regenerated)
}

pub fn output_type_from_input(value: &str) -> Result<OutputType, CliError> {
    if value.eq_ignore_ascii_case("mise") {
        Ok(OutputType::Mise)
    } else if matches!(value.to_ascii_lowercase().as_str(), "docker-compose" | "docker_compose") {
        Ok(OutputType::DockerCompose)
    } else {
        Err(CliError::UnsupportedDeploymentType(value.to_string()))
    }
}

const fn deployment_summary(
    cluster_name: String,
    agent_templates: usize,
    module_names: Vec<String>,
) -> DeploymentSummary {
    DeploymentSummary {
        cluster_name,
        agent_templates,
        module_names,
    }
}

fn generate_deployment_outputs(
    cluster: &ClusterInput,
    output_dir: &Path,
    output_types: &[OutputType],
) -> Result<(), CliError> {
    if !output_dir.exists() {
        fs::create_dir_all(output_dir)?;
    }

    for output_type in output_types {
        match output_type {
            OutputType::Mise => generate_mise_deployment(cluster, output_dir)?,
            OutputType::DockerCompose => generate_docker_compose_deployment(cluster, output_dir)?,
        }
    }

    let readme_path = output_dir.join("README.md");
    let module_names = cluster_module_names(cluster);
    fs::write(&readme_path, generated_readme(cluster, &module_names, output_types))?;

    Ok(())
}

fn discover_verification_scenarios(verification_root: &Path) -> Result<Vec<(PathBuf, PathBuf)>, CliError> {
    let mut scenarios = Vec::new();
    let verification_sets = fs::read_dir(verification_root)?;

    for entry in verification_sets {
        let entry = entry?;
        let set_root = entry.path();
        if !entry.file_type()?.is_dir() {
            continue;
        }

        let input_dir = set_root.join("input");
        let output_root = set_root.join("output");
        if !input_dir.is_dir() {
            continue;
        }

        let entries = fs::read_dir(&input_dir)?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            #[expect(
                clippy::filetype_is_file,
                reason = "scenario inputs are regular files only; dirs and symlinks are intentionally skipped"
            )]
            let is_regular_file = entry.file_type()?.is_file();
            if !is_regular_file {
                continue;
            }

            let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
                continue;
            };
            if !matches!(extension, "yaml" | "yml") {
                continue;
            }

            let Some(stem) = path.file_stem().map(PathBuf::from) else {
                return Err(CliError::MissingFileStem(path));
            };
            scenarios.push((path, output_root.join(stem)));
        }
    }

    if scenarios.is_empty() {
        return Err(CliError::NoScenarios(verification_root.to_path_buf()));
    }

    scenarios.sort_by(|(left, _), (right, _)| left.cmp(right));
    Ok(scenarios)
}

fn generated_readme(cluster: &ClusterInput, module_names: &[String], output_types: &[OutputType]) -> String {
    let module_summary = if module_names.is_empty() {
        "No workflow modules were selected in the scenario input.".to_string()
    } else {
        format!(
            "The scenario exposes these workflow modules: {}.",
            module_names.join(", ")
        )
    };

    let output_files = generated_output_files(output_types);
    let output_summary = if let [only_file] = output_files.as_slice() {
        format!(
            "This directory contains the generated `{only_file}` for the `{}` scenario.",
            cluster.cluster_name
        )
    } else {
        let output_files = output_files
            .iter()
            .map(|output_file| format!("`{output_file}`"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            concat!(
                "This directory contains generated deployment configs for the `{}` scenario.\n",
                "Files: {}.",
            ),
            cluster.cluster_name, output_files
        )
    };
    let run_instructions = output_types
        .iter()
        .map(|output_type| generated_run_instructions(*output_type))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        concat!(
            "# {name}\n\n",
            "{output_summary}\n\n",
            "{module_summary}\n\n",
            "{run_instructions}",
        ),
        name = cluster.cluster_name,
        output_summary = output_summary,
        module_summary = module_summary,
        run_instructions = run_instructions,
    )
}

fn generated_run_instructions(output_type: OutputType) -> String {
    match output_type {
        OutputType::Mise => concat!(
            "## Run With Mise\n\n",
            "From this directory, start the scenario with:\n\n",
            "```bash\n",
            "mise run generated-scenario\n",
            "```\n\n",
            "That task starts both OpenObserve and `ws-server` for this scenario.\n\n",
            "### Open The OpenObserve UI\n\n",
            "From this directory, open the OpenObserve UI with:\n\n",
            "```bash\n",
            "mise run open-o2\n",
            "```\n"
        )
        .to_string(),
        OutputType::DockerCompose => concat!(
            "## Run With Docker Compose\n\n",
            "From this directory, start the scenario with:\n\n",
            "```bash\n",
            "docker compose up --build\n",
            "```\n\n",
            "The compose stack starts OpenObserve and builds a `ws-server` image from the repository Dockerfile.\n",
            "`ws-server` runs with host networking so it advertises the same LAN IP as the `mise` deployment.\n\n",
            "### Open The UIs\n\n",
            "OpenObserve is available at <http://localhost:5080/>.\n",
            "`ws-server` is available at <http://localhost:8080/> and <https://localhost:8443/>.\n\n",
            "Stop the scenario with:\n\n",
            "```bash\n",
            "docker compose down\n",
            "```\n"
        )
        .to_string(),
    }
}

#[must_use]
pub fn module_registry(project_root: &Path, ws_server_dir: &Path) -> BTreeMap<String, ModuleRegistryEntry> {
    let mut registry = BTreeMap::new();

    register_modules_under(
        &mut registry,
        &project_root.join("services/ws-modules"),
        ws_server_dir,
        "/app/services/ws-modules",
    );
    register_modules_under(
        &mut registry,
        &project_root.join("data/model-modules"),
        ws_server_dir,
        "/app/data/model-modules",
    );
    // Generated Python ws-modules: each generated/python-{ws,rest}/ holds
    // its own pkg/package.json after `mise run build-et-{ws,rest-client}-
    // wheel`. They're listed individually because the parent `generated/`
    // also contains non-module artifacts (rust-rest, dart-ws, zig-rest,
    // specs, docs).
    register_module_at(
        &mut registry,
        &project_root.join("generated/python-ws"),
        ws_server_dir,
        "/app/generated/python-ws",
    );
    register_module_at(
        &mut registry,
        &project_root.join("generated/python-rest"),
        ws_server_dir,
        "/app/generated/python-rest",
    );

    register_external_module(
        &mut registry,
        "onnxruntime-web",
        "$(mise where npm:onnxruntime-web)/lib/node_modules/onnxruntime-web",
        "/app/node_modules/onnxruntime-web",
    );
    register_external_module(
        &mut registry,
        "pyodide",
        "$(mise where npm:pyodide)/lib/node_modules/pyodide",
        "/app/node_modules/pyodide",
    );

    registry
}

fn register_modules_under(
    registry: &mut BTreeMap<String, ModuleRegistryEntry>,
    root: &Path,
    ws_server_dir: &Path,
    docker_root: &str,
) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };

    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }

        let module_path = entry.path();
        let Some(directory_name) = module_path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        register_module(
            registry,
            &module_path,
            directory_name,
            ws_server_dir,
            &format!("{docker_root}/{directory_name}"),
        );
    }
}

/// Register a single module by its filesystem path (not a parent dir).
/// Used for modules that don't live under `services/ws-modules/` --
/// currently the generated python clients under `generated/`.
fn register_module_at(
    registry: &mut BTreeMap<String, ModuleRegistryEntry>,
    module_path: &Path,
    ws_server_dir: &Path,
    docker_path: &str,
) {
    let Some(directory_name) = module_path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    register_module(registry, module_path, directory_name, ws_server_dir, docker_path);
}

fn register_module(
    registry: &mut BTreeMap<String, ModuleRegistryEntry>,
    module_path: &Path,
    directory_name: &str,
    ws_server_dir: &Path,
    docker_path: &str,
) {
    let package = module_package_json(module_path);
    let entry = ModuleRegistryEntry {
        mise_path: relative_path_from(ws_server_dir, module_path),
        docker_path: docker_path.to_string(),
        dependencies: package
            .as_ref()
            .map(|package| package.dependencies.keys().cloned().collect())
            .unwrap_or_default(),
    };

    let _previous: Option<ModuleRegistryEntry> = registry.insert(directory_name.to_string(), entry.clone());
    if let Some(package_name) = package.and_then(|package| package.name) {
        let _previous: Option<ModuleRegistryEntry> = registry.insert(package_name, entry);
    }
}

fn register_external_module(
    registry: &mut BTreeMap<String, ModuleRegistryEntry>,
    package_name: &str,
    mise_path: &str,
    docker_path: &str,
) {
    let _previous: Option<ModuleRegistryEntry> = registry.insert(
        package_name.to_string(),
        ModuleRegistryEntry {
            mise_path: mise_path.to_string(),
            docker_path: docker_path.to_string(),
            dependencies: BTreeSet::new(),
        },
    );
}

#[must_use]
pub fn module_package_json(module_path: &Path) -> Option<PackageJson> {
    let pkg_package = read_package_json(&module_path.join("pkg/package.json"));
    let root_package = read_package_json(&module_path.join("package.json"));
    let pyproject = read_pyproject_package(&module_path.join("pyproject.toml"));
    let cargo_package = read_cargo_package(&module_path.join("Cargo.toml"));
    if pkg_package.is_none() && root_package.is_none() && pyproject.is_none() && cargo_package.is_none() {
        return None;
    }

    let mut package = pkg_package.or_else(|| root_package.clone()).unwrap_or_default();
    if let Some(root_package) = root_package {
        package.dependencies.extend(root_package.dependencies);
        if package.name.is_none() {
            package.name = root_package.name;
        }
    }
    if let Some(pyproject) = pyproject {
        if let Some(ws_module) = pyproject.tool.and_then(|tool| tool.ws_module) {
            package.dependencies.extend(ws_module.dependencies);
        }
        if package.name.is_none() {
            package.name = pyproject.project.and_then(|project| project.name);
        }
    }
    if let Some(cargo_package) = cargo_package.and_then(|cargo_package| cargo_package.package) {
        if let Some(ws_module) = cargo_package.metadata.and_then(|metadata| metadata.ws_module) {
            package.dependencies.extend(ws_module.dependencies);
        }
        if package.name.is_none() {
            package.name = cargo_package.name;
        }
    }
    Some(package)
}

fn read_package_json(path: &Path) -> Option<PackageJson> {
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn read_pyproject_package(path: &Path) -> Option<PyprojectPackage> {
    let content = fs::read_to_string(path).ok()?;
    toml::from_str(&content).ok()
}

fn read_cargo_package(path: &Path) -> Option<CargoPackage> {
    let content = fs::read_to_string(path).ok()?;
    toml::from_str(&content).ok()
}

pub fn resolve_module_paths<F>(
    registry: &BTreeMap<String, ModuleRegistryEntry>,
    module_names: &[String],
    path_for: F,
) -> Result<Vec<String>, CliError>
where
    F: Fn(&ModuleRegistryEntry) -> String,
{
    let mut paths = Vec::new();
    let mut queued: VecDeque<String> = module_names.iter().cloned().collect();
    let mut seen_keys = BTreeSet::new();
    let mut seen_paths = BTreeSet::new();

    while let Some(module_name) = queued.pop_front() {
        if !seen_keys.insert(module_name.clone()) {
            continue;
        }

        let entry = registry
            .get(&module_name)
            .ok_or_else(|| CliError::UnknownDependency(module_name.clone()))?;
        let path = path_for(entry);
        if seen_paths.insert(path.clone()) {
            paths.push(path);
        }
        queued.extend(entry.dependencies.iter().cloned());
    }

    Ok(paths)
}

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

#[must_use]
pub fn cluster_module_names(cluster: &ClusterInput) -> Vec<String> {
    cluster
        .agents
        .iter()
        .flat_map(|agent| {
            agent
                .resources
                .iter()
                .map(|resource| resource.resource_type.trim().to_string())
                .filter(|module_name| !module_name.is_empty())
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
