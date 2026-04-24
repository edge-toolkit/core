use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use clap::ValueEnum;
use edge_toolkit::input::ClusterInput;
use serde::Deserialize;
use toml::{Table, Value};

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
pub struct DeploymentSummary {
    pub cluster_name: String,
    pub agent_templates: usize,
    pub module_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegeneratedScenario {
    pub input_file: PathBuf,
    pub output_dir: PathBuf,
    pub summary: DeploymentSummary,
}

pub fn generate_deployment(
    input_file: &Path,
    output_dir: &Path,
    output_type: Option<OutputType>,
) -> Result<DeploymentSummary> {
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

pub fn load_cluster_input(input_file: &Path) -> Result<ClusterInput> {
    let content =
        fs::read_to_string(input_file).with_context(|| format!("Failed to read input file: {:?}", input_file))?;

    serde_yaml::from_str(&content).with_context(|| "Failed to parse cluster input YAML")
}

pub fn regenerate_verification(
    verification_root: &Path,
    output_type: Option<OutputType>,
) -> Result<Vec<RegeneratedScenario>> {
    let scenarios = discover_verification_scenarios(verification_root)?;

    let mut regenerated = Vec::with_capacity(scenarios.len());
    let mut seen_output_dirs = BTreeSet::new();
    for (input_file, output_dir) in scenarios {
        if !seen_output_dirs.insert(output_dir.clone()) {
            return Err(anyhow!(
                "Verification root {:?} maps multiple scenario inputs to the same output directory {:?}",
                verification_root,
                output_dir
            ));
        }
        let cluster = load_cluster_input(&input_file)?;
        let module_names = cluster_module_names(&cluster);
        let output_types = match &output_type {
            Some(output_type) => std::slice::from_ref(output_type),
            None => OutputType::ALL,
        };

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

pub fn output_type_from_input(value: &str) -> Result<OutputType> {
    if value.eq_ignore_ascii_case("mise") {
        Ok(OutputType::Mise)
    } else if matches!(value.to_ascii_lowercase().as_str(), "docker-compose" | "docker_compose") {
        Ok(OutputType::DockerCompose)
    } else {
        Err(anyhow!(
            "Unsupported deployment_type {:?}. Supported values are currently: mise, docker-compose",
            value
        ))
    }
}

fn deployment_summary(cluster_name: String, agent_templates: usize, module_names: Vec<String>) -> DeploymentSummary {
    DeploymentSummary {
        cluster_name,
        agent_templates,
        module_names,
    }
}

fn generate_deployment_outputs(cluster: &ClusterInput, output_dir: &Path, output_types: &[OutputType]) -> Result<()> {
    if !output_dir.exists() {
        fs::create_dir_all(output_dir)
            .with_context(|| format!("Failed to create output directory: {:?}", output_dir))?;
    }

    for output_type in output_types {
        match output_type {
            OutputType::Mise => generate_mise_deployment(cluster, output_dir)?,
            OutputType::DockerCompose => generate_docker_compose_deployment(cluster, output_dir)?,
        }
    }

    let readme_path = output_dir.join("README.md");
    let module_names = cluster_module_names(cluster);
    fs::write(&readme_path, generated_readme(cluster, &module_names, output_types))
        .with_context(|| format!("Failed to write output file: {:?}", readme_path))?;

    Ok(())
}

fn discover_verification_scenarios(verification_root: &Path) -> Result<Vec<(PathBuf, PathBuf)>> {
    let mut scenarios = Vec::new();
    let verification_sets = fs::read_dir(verification_root)
        .with_context(|| format!("Failed to read verification root directory: {:?}", verification_root))?;

    for entry in verification_sets {
        let entry = entry.with_context(|| format!("Failed to read entry from {:?}", verification_root))?;
        let set_root = entry.path();
        if !entry
            .file_type()
            .with_context(|| format!("Failed to read file type for {:?}", set_root))?
            .is_dir()
        {
            continue;
        }

        let input_dir = set_root.join("input");
        let output_root = set_root.join("output");
        if !input_dir.is_dir() {
            continue;
        }

        let entries = fs::read_dir(&input_dir)
            .with_context(|| format!("Failed to read verification input directory: {:?}", input_dir))?;
        for entry in entries {
            let entry = entry.with_context(|| format!("Failed to read entry from {:?}", input_dir))?;
            let path = entry.path();
            if !entry
                .file_type()
                .with_context(|| format!("Failed to read file type for {:?}", path))?
                .is_file()
            {
                continue;
            }

            let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
                continue;
            };
            if !matches!(extension, "yaml" | "yml") {
                continue;
            }

            let Some(stem) = path.file_stem().map(PathBuf::from) else {
                return Err(anyhow!("Verification input file {:?} has no file stem", path));
            };
            scenarios.push((path, output_root.join(stem)));
        }
    }

    if scenarios.is_empty() {
        return Err(anyhow!(
            "Verification root {:?} does not contain any scenario files under */input/*.yaml or */input/*.yml",
            verification_root
        ));
    }

    scenarios.sort_by(|(left, _), (right, _)| left.cmp(right));
    Ok(scenarios)
}

fn generate_mise_deployment(cluster: &ClusterInput, output_dir: &Path) -> Result<()> {
    let output_path = output_dir.join("mise.toml");
    let workspace_root =
        std::env::current_dir().with_context(|| "Failed to resolve current working directory for mise tasks")?;
    let output_abs = absolute_from(&workspace_root, output_dir);
    let ws_server_dir = workspace_root.join("services/ws-server");
    let workspace_rel = relative_path_from(&output_abs, &workspace_root).display().to_string();
    let openobserve_env_file_rel = "config/o2.env";
    let module_names = cluster_module_names(cluster);
    let module_paths = scenario_module_paths(&ws_server_dir, &module_names);
    let module_paths_lines = module_paths
        .iter()
        .map(|p| format!("  {p}"))
        .collect::<Vec<_>>()
        .join(",\\\n");
    let ws_server_run = format!(
        "export MODULES_PATHS=\"\\\n{},\\\n  $(mise where npm:onnxruntime-web)/lib/node_modules\"\ncargo run\n",
        module_paths_lines
    );
    let ws_server_rel = relative_path_from(&output_abs, &ws_server_dir).display().to_string();

    let mut root = Table::new();
    let mut tasks = Table::new();

    tasks.insert(
        "openobserve".to_string(),
        Value::Table(mise_task(
            Some("o2"),
            None,
            Some(&workspace_rel),
            Some(&format!(
                "docker run --rm -it --name openobserve -p 5080:5080 --env-file {} openobserve/openobserve:v0.70.3",
                openobserve_env_file_rel
            )),
            None,
            None,
        )),
    );
    tasks.insert(
        "ws-server".to_string(),
        Value::Table(mise_task(
            None,
            Some("Run the WebSocket server"),
            Some(&ws_server_rel),
            Some(&ws_server_run),
            None,
            Some(mise_env()),
        )),
    );
    tasks.insert(
        "generated-scenario".to_string(),
        Value::Table(mise_task(
            None,
            Some(&format!("Run generated scenario for {}", cluster.cluster_name)),
            None,
            None,
            Some(mise_depends(["openobserve", "ws-server"])),
            None,
        )),
    );
    tasks.insert(
        "open-o2".to_string(),
        Value::Table(mise_task(
            None,
            Some("Open the OpenObserve UI"),
            None,
            Some("open http://localhost:5080/"),
            None,
            None,
        )),
    );

    root.insert("tasks".to_string(), Value::Table(tasks));

    let content = format_mise_toml(
        toml::to_string(&Value::Table(root)).context("Failed to serialize mise TOML")?,
        openobserve_env_file_rel,
    );
    fs::write(&output_path, content).with_context(|| format!("Failed to write output file: {:?}", output_path))?;

    Ok(())
}

fn generate_docker_compose_deployment(cluster: &ClusterInput, output_dir: &Path) -> Result<()> {
    let output_path = output_dir.join(OutputType::DockerCompose.output_file_name());
    let workspace_root =
        std::env::current_dir().with_context(|| "Failed to resolve current working directory for compose services")?;
    let output_abs = absolute_from(&workspace_root, output_dir);
    let workspace_rel = relative_path_from(&output_abs, &workspace_root).display().to_string();
    let openobserve_env_file_rel = relative_path_from(&output_abs, &workspace_root.join("config/o2.env"))
        .display()
        .to_string();
    let module_names = cluster_module_names(cluster);
    let module_paths = docker_image_module_paths(&module_names);
    let compose = ComposeFile {
        services: vec![
            (
                "openobserve".to_string(),
                ComposeService {
                    image: Some("openobserve/openobserve:v0.70.3".to_string()),
                    healthcheck: Some(ComposeHealthcheck {
                        test: vec![
                            "CMD".to_string(),
                            "/openobserve".to_string(),
                            "node".to_string(),
                            "status".to_string(),
                        ],
                        interval: "5s".to_string(),
                        timeout: "3s".to_string(),
                        retries: 20,
                        start_period: "10s".to_string(),
                    }),
                    ports: vec!["5080:5080".to_string()],
                    env_file: vec![openobserve_env_file_rel],
                    environment: vec![("ZO_DATA_DIR".to_string(), ComposeValue::Plain("/data".to_string()))],
                    volumes: vec!["openobserve-data:/data".to_string()],
                    ..ComposeService::default()
                },
            ),
            (
                "ws-server".to_string(),
                ComposeService {
                    build: Some(ComposeBuild {
                        context: workspace_rel,
                        dockerfile: "services/ws-server/Dockerfile".to_string(),
                    }),
                    network_mode: Some("host".to_string()),
                    environment: vec![
                        (
                            "MODULES_PATHS".to_string(),
                            ComposeValue::WrappedDoubleQuoted(module_paths),
                        ),
                        (
                            "OTLP_AUTH_PASSWORD".to_string(),
                            ComposeValue::DoubleQuoted("1234".to_string()),
                        ),
                        (
                            "OTLP_AUTH_USERNAME".to_string(),
                            ComposeValue::Plain("root@example.com".to_string()),
                        ),
                        (
                            "OTLP_COLLECTOR_URL".to_string(),
                            ComposeValue::Plain("http://127.0.0.1:5080/api/default/v1".to_string()),
                        ),
                        (
                            "STORAGE_PATH".to_string(),
                            ComposeValue::Plain("/app/storage".to_string()),
                        ),
                    ],
                    volumes: vec!["ws-server-storage:/app/storage".to_string()],
                    depends_on: vec![(
                        "openobserve".to_string(),
                        ComposeDependsOnCondition {
                            condition: "service_healthy".to_string(),
                        },
                    )],
                    ..ComposeService::default()
                },
            ),
        ],
        volumes: vec![
            ("openobserve-data".to_string(), ComposeVolume),
            ("ws-server-storage".to_string(), ComposeVolume),
        ],
    };
    let content = render_compose_yaml(&compose);
    fs::write(&output_path, content).with_context(|| format!("Failed to write output file: {:?}", output_path))?;

    Ok(())
}

#[derive(Debug, Default)]
struct ComposeFile {
    services: Vec<(String, ComposeService)>,
    volumes: Vec<(String, ComposeVolume)>,
}

#[derive(Debug, Default)]
struct ComposeService {
    build: Option<ComposeBuild>,
    image: Option<String>,
    healthcheck: Option<ComposeHealthcheck>,
    network_mode: Option<String>,
    ports: Vec<String>,
    env_file: Vec<String>,
    environment: Vec<(String, ComposeValue)>,
    volumes: Vec<String>,
    depends_on: Vec<(String, ComposeDependsOnCondition)>,
}

#[derive(Debug)]
struct ComposeBuild {
    context: String,
    dockerfile: String,
}

#[derive(Debug)]
struct ComposeHealthcheck {
    test: Vec<String>,
    interval: String,
    timeout: String,
    retries: u32,
    start_period: String,
}

#[derive(Debug)]
struct ComposeDependsOnCondition {
    condition: String,
}

#[derive(Debug, Default)]
struct ComposeVolume;

#[derive(Debug)]
enum ComposeValue {
    Plain(String),
    DoubleQuoted(String),
    WrappedDoubleQuoted(Vec<String>),
}

fn render_compose_yaml(compose: &ComposeFile) -> String {
    let mut renderer = ComposeRenderer::default();
    renderer.push_line(0, "services:");
    for (name, service) in &compose.services {
        renderer.render_service(name, service);
    }
    renderer.push_line(0, "volumes:");
    for (name, _) in &compose.volumes {
        renderer.push_line(1, &format!("{name}: {{}}"));
    }
    renderer.finish()
}

#[derive(Default)]
struct ComposeRenderer {
    output: String,
}

impl ComposeRenderer {
    fn finish(self) -> String {
        self.output
    }

    fn push_line(&mut self, indent: usize, line: &str) {
        self.output.push_str(&"  ".repeat(indent));
        self.output.push_str(line);
        self.output.push('\n');
    }

    fn render_service(&mut self, name: &str, service: &ComposeService) {
        self.push_line(1, &format!("{name}:"));
        if let Some(image) = &service.image {
            self.push_line(2, &format!("image: {image}"));
        }
        if let Some(healthcheck) = &service.healthcheck {
            self.push_line(2, "healthcheck:");
            self.push_line(3, "test:");
            for item in &healthcheck.test {
                self.push_line(4, &format!("- {item}"));
            }
            self.push_line(3, &format!("interval: {}", healthcheck.interval));
            self.push_line(3, &format!("timeout: {}", healthcheck.timeout));
            self.push_line(3, &format!("retries: {}", healthcheck.retries));
            self.push_line(3, &format!("start_period: {}", healthcheck.start_period));
        }
        if !service.ports.is_empty() {
            self.push_line(2, "ports:");
            for port in &service.ports {
                self.push_line(3, &format!("- {port}"));
            }
        }
        if !service.env_file.is_empty() {
            self.push_line(2, "env_file:");
            for env_file in &service.env_file {
                self.push_line(3, &format!("- {env_file}"));
            }
        }
        if let Some(build) = &service.build {
            self.push_line(2, "build:");
            self.push_line(3, &format!("context: {}", build.context));
            self.push_line(3, &format!("dockerfile: {}", build.dockerfile));
        }
        if let Some(network_mode) = &service.network_mode {
            self.push_line(2, &format!("network_mode: {network_mode}"));
        }
        if !service.environment.is_empty() {
            self.push_line(2, "environment:");
            for (key, value) in &service.environment {
                self.render_environment_value(key, value);
            }
        }
        if !service.volumes.is_empty() {
            self.push_line(2, "volumes:");
            for volume in &service.volumes {
                self.push_line(3, &format!("- {volume}"));
            }
        }
        if !service.depends_on.is_empty() {
            self.push_line(2, "depends_on:");
            for (name, condition) in &service.depends_on {
                self.push_line(3, &format!("{name}:"));
                self.push_line(4, &format!("condition: {}", condition.condition));
            }
        }
    }

    fn render_environment_value(&mut self, key: &str, value: &ComposeValue) {
        match value {
            ComposeValue::Plain(value) => self.push_line(3, &format!("{key}: {value}")),
            ComposeValue::DoubleQuoted(value) => self.push_line(3, &format!("{key}: \"{value}\"")),
            ComposeValue::WrappedDoubleQuoted(parts) => {
                if let Some((first, rest)) = parts.split_first() {
                    self.push_line(3, &format!("{key}: \"{first},\\"));
                    for (index, part) in rest.iter().enumerate() {
                        let suffix = if index + 1 == rest.len() { "\"" } else { ",\\" };
                        self.push_line(4, &format!("{part}{suffix}"));
                    }
                } else {
                    self.push_line(3, &format!("{key}: \"\""));
                }
            }
        }
    }
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
    let output_summary = if output_files.len() == 1 {
        format!(
            "This directory contains the generated `{}` for the `{}` scenario.",
            output_files[0], cluster.cluster_name
        )
    } else {
        let output_files = output_files
            .iter()
            .map(|output_file| format!("`{}`", output_file))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "This directory contains generated deployment configs for the `{}` scenario.\n\
Files: {}.",
            cluster.cluster_name, output_files
        )
    };
    let run_instructions = output_types
        .iter()
        .map(|output_type| generated_run_instructions(*output_type))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "# {name}\n\n\
{output_summary}\n\n\
{module_summary}\n\n\
{run_instructions}",
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

fn format_mise_toml(content: String, openobserve_env_file_rel: &str) -> String {
    let openobserve_run = format!(
        concat!(
            "run = \"docker run --rm -it --name openobserve -p 5080:5080 --env-file {} ",
            "openobserve/openobserve:v0.70.3\""
        ),
        openobserve_env_file_rel
    );
    let wrapped_openobserve_run = format!(
        concat!(
            "run = \"\"\"\n",
            "docker run --rm --name openobserve -p 5080:5080 \\\n",
            "  --env-file {} \\\n",
            "  openobserve/openobserve:v0.70.3\n",
            "\"\"\""
        ),
        openobserve_env_file_rel
    );
    content.replace(&openobserve_run, &wrapped_openobserve_run)
}

fn mise_task(
    alias: Option<&str>,
    description: Option<&str>,
    dir: Option<&str>,
    run: Option<&str>,
    extra: Option<Table>,
    env: Option<Table>,
) -> Table {
    let mut task = Table::new();
    if let Some(alias) = alias {
        task.insert("alias".to_string(), Value::String(alias.to_string()));
    }
    if let Some(description) = description {
        task.insert("description".to_string(), Value::String(description.to_string()));
    }
    if let Some(dir) = dir {
        task.insert("dir".to_string(), Value::String(dir.to_string()));
    }
    if let Some(run) = run {
        task.insert("run".to_string(), Value::String(run.to_string()));
    }
    if let Some(extra) = extra {
        for (key, value) in extra {
            task.insert(key, value);
        }
    }
    if let Some(env) = env {
        task.insert("env".to_string(), Value::Table(env));
    }
    task
}

fn mise_env() -> Table {
    let mut env = Table::new();
    env.insert("OTLP_AUTH_PASSWORD".to_string(), Value::String("1234".to_string()));
    env.insert(
        "OTLP_AUTH_USERNAME".to_string(),
        Value::String("root@example.com".to_string()),
    );
    env
}

fn mise_depends<const N: usize>(depends: [&str; N]) -> Table {
    let mut extra = Table::new();
    extra.insert(
        "depends".to_string(),
        Value::Array(
            depends
                .into_iter()
                .map(|dependency| Value::String(dependency.to_string()))
                .collect(),
        ),
    );
    extra
}

fn scenario_module_paths(ws_server_dir: &Path, module_names: &[String]) -> Vec<String> {
    let project_root = edge_toolkit::config::get_project_root();
    let ws_modules_dir = project_root.join("services/ws-modules");
    let mut paths = vec![
        relative_path_from(ws_server_dir, &project_root.join("services/ws-server/static"))
            .display()
            .to_string(),
        relative_path_from(ws_server_dir, &project_root.join("services/ws-wasm-agent"))
            .display()
            .to_string(),
        relative_path_from(ws_server_dir, &project_root.join("data/model-modules"))
            .display()
            .to_string(),
    ];
    for module_name in module_names {
        paths.push(
            relative_path_from(ws_server_dir, &ws_modules_dir.join(module_name))
                .display()
                .to_string(),
        );
    }
    paths
}

fn docker_image_module_paths(module_names: &[String]) -> Vec<String> {
    let mut paths = Vec::with_capacity(module_names.len() + 4);
    paths.push("/app/services/ws-server/static".to_string());
    paths.push("/app/services/ws-wasm-agent".to_string());
    paths.push("/app/data/model-modules".to_string());
    paths.push("/app/node_modules/onnxruntime-web".to_string());
    for module_name in module_names {
        paths.push(format!("/app/services/ws-modules/{module_name}"));
    }
    paths
}

fn absolute_from(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        normalize_path(path)
    } else {
        normalize_path(&base.join(path))
    }
}

fn relative_path_from(from_dir: &Path, target: &Path) -> PathBuf {
    let from_components = normal_components(&normalize_path(from_dir));
    let target_components = normal_components(&normalize_path(target));
    let common_len = from_components
        .iter()
        .zip(target_components.iter())
        .take_while(|(from, target)| from == target)
        .count();

    let mut relative = PathBuf::new();
    for _ in common_len..from_components.len() {
        relative.push("..");
    }
    for component in target_components.iter().skip(common_len) {
        relative.push(component);
    }

    if relative.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        relative
    }
}

fn normal_components(path: &Path) -> Vec<OsString> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_os_string()),
            _ => None,
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
                normalized.pop();
            }
            Component::Normal(value) => normalized.push(value),
        }
    }
    normalized
}

fn cluster_module_names(cluster: &ClusterInput) -> Vec<String> {
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

#[cfg(test)]
mod tests;
