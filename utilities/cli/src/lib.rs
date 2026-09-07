#![expect(
    clippy::single_call_fn,
    reason = "et-cli decomposes scenario generation into named pipeline stages; each invoked once for readability"
)]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use clap::ValueEnum;
use edge_toolkit::input::ClusterInput;
use et_path::relative_path_from;
use fs_err as fs;
use serde::Deserialize;

mod deployment_types;
mod error;
mod module_package_json;
mod scenario_password;

pub use self::deployment_types::{
    docker_image_module_paths, generate_docker_compose_deployment, generate_mise_deployment, generate_scenario_image,
    scenario_module_paths,
};
pub use self::error::CliError;
pub use self::module_package_json::generate_module_package_json;
pub use self::scenario_password::{scenario_password, scenario_seed};

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

/// Where a module's served files come from.
///
/// The deployment generators need this to tell apart the two provisioning routes: a repo directory can be
/// copied straight out of the Docker build context, whereas a mise-staged package exists only in the tool's
/// install dir and has to be installed before it can be staged.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ModuleSource {
    /// A directory in this repository, as a path relative to the repository root.
    Repo(String),
    /// A package staged by a mise tool.
    ///
    /// Holds the backend-qualified tool id plus the published package name to locate beneath its install
    /// directory. The name rather than a path because the npm backend has no single layout: a package lands
    /// under `lib/node_modules/`, `node_modules/`, or an aube virtual store keyed by a content hash,
    /// depending on backend and platform, so the directory has to be found rather than assumed.
    MiseTool { tool: String, package: String },
}

/// Where the hub serves pyodide from, whichever distribution was selected.
const PYODIDE_DOCKER_PATH: &str = "/app/node_modules/pyodide";

/// Pragma marking a generated credential as a deliberate, dev-only literal.
///
/// The scenario password is derived from the scenario input so a deployment is reproducible, which means it is
/// written into files that are committed, which means the secret scanner finds it. It is a local dev credential
/// for a collector nothing outside the developer's machine talks to, so the finding is suppressed per line rather
/// than by excluding the tree -- `.deepsource.toml` already excludes `verification/**`, and the secrets analyzer
/// scans it regardless.
pub const SECRET_PRAGMA: &str = "# skipcq: SCT-A000 -- generated dev-only scenario credential";

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ModuleRegistryEntry {
    pub mise_path: String,
    pub docker_path: String,
    pub dependencies: BTreeSet<String>,
    pub source: ModuleSource,
}

pub fn generate_deployment(
    input_file: &Path,
    output_dir: &Path,
    output_type: Option<OutputType>,
) -> Result<DeploymentSummary, CliError> {
    let (cluster, seed) = load_cluster_input(input_file)?;
    let output_type = output_type
        .map(Ok)
        .or_else(|| cluster.deployment_type.as_deref().map(output_type_from_input))
        .unwrap_or(Ok(OutputType::Mise))?;

    let module_names = cluster_module_names(&cluster);
    generate_deployment_outputs(&cluster, output_dir, &[output_type], seed)?;

    Ok(deployment_summary(
        cluster.cluster_name,
        cluster.agents.len(),
        module_names,
    ))
}

/// Load a scenario input together with the RNG seed derived from its bytes.
///
/// Read once and hashed here so the seed covers the exact file the deployment was generated from, comments and
/// formatting included, rather than a re-serialization of the parsed struct.
pub fn load_cluster_input(input_file: &Path) -> Result<(ClusterInput, u64), CliError> {
    let content = fs::read(input_file)?;
    let cluster = serde_yaml::from_slice(&content)?;

    Ok((cluster, scenario_seed(&content)))
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

        let (cluster, seed) = load_cluster_input(&input_file)?;
        let module_names = cluster_module_names(&cluster);
        let output_types = output_type.as_ref().map_or(OutputType::ALL, std::slice::from_ref);

        generate_deployment_outputs(&cluster, &output_dir, output_types, seed)?;
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
    seed: u64,
) -> Result<(), CliError> {
    if !output_dir.exists() {
        fs::create_dir_all(output_dir)?;
    }

    // One password per scenario, shared by both deployment formats.
    // OpenObserve and the ws-server have to agree on it: the server authenticates its OTLP exports against the
    // same root credentials the collector was started with.
    let password = scenario_password(seed);
    for output_type in output_types {
        match output_type {
            OutputType::Mise => generate_mise_deployment(cluster, output_dir, &password)?,
            OutputType::DockerCompose => {
                generate_docker_compose_deployment(cluster, output_dir, &password)?;
                generate_scenario_image(cluster, output_dir)?;
            }
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
            if extension != "yaml" {
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
            "The compose stack starts OpenObserve and builds `ws-server` in two layers: the module-less hub\n",
            "image from the repository's `services/ws-server/Dockerfile`, then the `Dockerfile` in this\n",
            "directory, which stages this scenario's modules onto it. The hub is build-only and never runs as a\n",
            "container of its own.\n",
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
        "npm:onnxruntime-web",
        "/app/node_modules/onnxruntime-web",
    );
    // Registered as the full distribution, which `resolve_cluster_modules` narrows to the much smaller npm
    // package for a cluster whose modules never call `micropip.install`. The full one comes from a GitHub
    // release tarball that mise's http backend extracts flat, so its install dir is itself the module directory.
    register_external_module(&mut registry, "pyodide", "http:pyodide", PYODIDE_DOCKER_PATH);

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
    // The docker path is always the repo-relative path under `/app`, which is where the hub image roots its
    // module scan, so stripping that prefix recovers the path to copy out of the build context.
    let repo_path = docker_path.strip_prefix("/app/").unwrap_or(docker_path).to_string();
    let entry = ModuleRegistryEntry {
        mise_path: relative_path_from(ws_server_dir, module_path),
        docker_path: docker_path.to_string(),
        dependencies: package
            .as_ref()
            .map(|package| package.dependencies.keys().cloned().collect())
            .unwrap_or_default(),
        source: ModuleSource::Repo(repo_path),
    };

    let _previous: Option<ModuleRegistryEntry> = registry.insert(directory_name.to_string(), entry.clone());
    if let Some(package_name) = package.and_then(|package| package.name) {
        let _previous: Option<ModuleRegistryEntry> = registry.insert(package_name, entry);
    }
}

/// Register a package that mise stages outside the repository, keyed by its published package name.
///
/// The mise path is a shell substitution rather than a literal, because where a tool's install dir keeps the
/// package is not knowable when the deployment is generated. An archive-backed `http:` tool extracts flat, so
/// `mise where` is already the answer; the npm backend spreads packages across several layouts that differ by
/// platform, so that case defers to `et-cli npm-module-path`, which resolves it through the same code the
/// ws-server uses to find these packages itself.
fn register_external_module(
    registry: &mut BTreeMap<String, ModuleRegistryEntry>,
    package_name: &str,
    tool: &str,
    docker_path: &str,
) {
    let entry = external_module_entry(package_name, tool, docker_path);
    let _previous: Option<ModuleRegistryEntry> = registry.insert(package_name.to_string(), entry);
}

/// Build the registry entry for a mise-staged package.
///
/// Separate from registration so the pyodide swap can rebuild an entry for a different tool without restating
/// how a mise path is spelled.
fn external_module_entry(package_name: &str, tool: &str, docker_path: &str) -> ModuleRegistryEntry {
    let mise_path = if tool.starts_with("npm:") {
        format!("$(cargo run --quiet -p et-cli -- npm-module-path --package {package_name})")
    } else {
        format!("$(mise where {tool})")
    };

    ModuleRegistryEntry {
        mise_path,
        docker_path: docker_path.to_string(),
        dependencies: BTreeSet::new(),
        source: ModuleSource::MiseTool {
            tool: tool.to_string(),
            package: package_name.to_string(),
        },
    }
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

/// Walk `module_names` and everything they depend on, in breadth-first declaration order.
///
/// A module is registered under both its directory name and its `package.json` name, so the same entry is
/// reachable by two keys; de-duplicating on the docker path collapses those without disturbing the order the
/// generated files depend on.
fn resolve_module_entries<'registry>(
    registry: &'registry BTreeMap<String, ModuleRegistryEntry>,
    module_names: &[String],
) -> Result<Vec<&'registry ModuleRegistryEntry>, CliError> {
    let mut entries = Vec::new();
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
        if seen_paths.insert(entry.docker_path.clone()) {
            entries.push(entry);
        }
        queued.extend(entry.dependencies.iter().cloned());
    }

    Ok(entries)
}

pub fn resolve_module_paths<F>(
    registry: &BTreeMap<String, ModuleRegistryEntry>,
    module_names: &[String],
    path_for: F,
) -> Result<Vec<String>, CliError>
where
    F: Fn(&ModuleRegistryEntry) -> String,
{
    Ok(resolve_cluster_modules(registry, module_names)?
        .iter()
        .map(path_for)
        .collect())
}

/// Resolve the cluster's modules to the docker path each is served from and how it is provisioned.
pub fn resolve_module_sources(
    registry: &BTreeMap<String, ModuleRegistryEntry>,
    module_names: &[String],
) -> Result<Vec<(String, ModuleSource)>, CliError> {
    Ok(resolve_cluster_modules(registry, module_names)?
        .into_iter()
        .map(|entry| (entry.docker_path, entry.source))
        .collect())
}

/// Resolve a cluster's modules, sized to what those modules actually need.
///
/// Everything is taken from the registry as-is except pyodide, whose distribution depends on the cluster: the
/// registry cannot decide that, because pyodide arrives as a dependency of whichever Python modules the cluster
/// happens to declare.
pub fn resolve_cluster_modules(
    registry: &BTreeMap<String, ModuleRegistryEntry>,
    module_names: &[String],
) -> Result<Vec<ModuleRegistryEntry>, CliError> {
    let resolved = resolve_module_entries(registry, module_names)?;
    let project_root = edge_toolkit::config::get_project_root();
    let needs_full_pyodide = resolved
        .iter()
        .any(|entry| module_installs_wheels(&project_root, entry));

    Ok(resolved
        .into_iter()
        .map(|entry| {
            if entry.docker_path == PYODIDE_DOCKER_PATH && !needs_full_pyodide {
                external_module_entry("pyodide", "npm:pyodide", PYODIDE_DOCKER_PATH)
            } else {
                entry.clone()
            }
        })
        .collect())
}

/// Whether a module pulls a non-stdlib wheel at runtime.
///
/// Decided from the module's served `pkg/`, which is the code the browser actually runs, rather than from its
/// Python sources: the `micropip.install` calls live in each Python module's JS loader shim.
fn module_installs_wheels(project_root: &Path, entry: &ModuleRegistryEntry) -> bool {
    let ModuleSource::Repo(repo_path) = &entry.source else {
        return false;
    };
    let Ok(files) = fs::read_dir(project_root.join(repo_path).join("pkg")) else {
        return false;
    };

    files.flatten().any(|file| {
        let path = file.path();
        if path.extension().is_none_or(|extension| extension != "js") {
            return false;
        }
        fs::read_to_string(&path).is_ok_and(|source| source.contains("micropip") && source.contains(".install("))
    })
}

/// Resolve the directory holding a mise-staged npm package.
///
/// Defers to the resolver the ws-server itself uses, which is the only place that knows the layouts mise's npm
/// backend produces. Generated deployments call back into this rather than embedding a path, because the layout
/// differs per platform and backend and so cannot be decided when the deployment is generated.
pub fn npm_module_path(package: &str) -> Result<PathBuf, CliError> {
    edge_toolkit::config::mise_npm_package_path(package)
        .ok_or_else(|| CliError::UnresolvedNpmModule(package.to_string()))
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
