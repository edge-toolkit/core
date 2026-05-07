use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use edge_toolkit::input::ClusterInput;

use crate::{
    OutputType, absolute_from, cluster_module_names, module_registry, relative_path_from, resolve_module_paths,
};

pub fn generate_docker_compose_deployment(cluster: &ClusterInput, output_dir: &Path) -> Result<()> {
    let output_path = output_dir.join(OutputType::DockerCompose.output_file_name());
    let workspace_root =
        std::env::current_dir().with_context(|| "Failed to resolve current working directory for compose services")?;
    let output_abs = absolute_from(&workspace_root, output_dir);
    let workspace_rel = relative_path_from(&output_abs, &workspace_root).display().to_string();
    let openobserve_env_file_rel = relative_path_from(&output_abs, &workspace_root.join("config/o2.env"))
        .display()
        .to_string();
    let module_names = cluster_module_names(cluster);
    let module_paths = docker_image_module_paths(&module_names)?;
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

pub fn docker_image_module_paths(module_names: &[String]) -> Result<Vec<String>> {
    let project_root = edge_toolkit::config::get_project_root();
    let ws_server_dir = project_root.join("services/ws-server");
    let mut paths = Vec::with_capacity(module_names.len() + 2);
    paths.push("/app/services/ws-server/static".to_string());
    paths.push("/app/services/ws-wasm-agent".to_string());
    let registry = module_registry(&project_root, &ws_server_dir);
    paths.extend(resolve_module_paths(&registry, module_names, |entry| {
        entry.docker_path.clone()
    })?);
    Ok(paths)
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
