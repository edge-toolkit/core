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

    if !output_dir.exists() {
        fs::create_dir_all(output_dir)
            .with_context(|| format!("Failed to create output directory: {:?}", output_dir))?;
    }

    let module_names = cluster_module_names(&cluster);

    match output_type {
        OutputType::Mise => generate_mise_deployment(&cluster, output_dir)?,
    }

    Ok(DeploymentSummary {
        cluster_name: cluster.cluster_name,
        agent_templates: cluster.agents.len(),
        module_names,
    })
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
        let summary = generate_deployment(&input_file, &output_dir, output_type)?;
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
    } else {
        Err(anyhow!(
            "Unsupported deployment_type {:?}. Supported values are currently: mise",
            value
        ))
    }
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
    let readme_path = output_dir.join("README.md");
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
        "MODULES_PATHS=\"\\\n{},\\\n  $(mise where npm:onnxruntime-web)/lib/node_modules\"\ncargo run\n",
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
    fs::write(&readme_path, generated_readme(cluster, &module_names))
        .with_context(|| format!("Failed to write output file: {:?}", readme_path))?;

    Ok(())
}

fn generated_readme(cluster: &ClusterInput, module_names: &[String]) -> String {
    let module_summary = if module_names.is_empty() {
        "No workflow modules were selected in the scenario input.".to_string()
    } else {
        format!(
            "The scenario exposes these workflow modules: {}.",
            module_names.join(", ")
        )
    };

    format!(
        "# {name}\n\n\
This directory contains the generated `mise.toml` for the `{name}` scenario.\n\n\
{module_summary}\n\n\
## Run The Scenario\n\n\
From this directory, start the scenario with:\n\n\
```bash\n\
mise run generated-scenario\n\
```\n\n\
That task starts both OpenObserve and `ws-server` for this scenario.\n\n\
## Open The OpenObserve UI\n\n\
From this directory, open the OpenObserve UI with:\n\n\
```bash\n\
mise run open-o2\n\
```\n",
        name = cluster.cluster_name,
        module_summary = module_summary,
    )
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
    let mut paths: Vec<String> = edge_toolkit::config::default_modules_folders()
        .into_iter()
        .filter(|p| p != &ws_modules_dir && p.starts_with(&project_root))
        .map(|p| relative_path_from(ws_server_dir, &p).display().to_string())
        .collect();
    for module_name in module_names {
        paths.push(
            relative_path_from(ws_server_dir, &ws_modules_dir.join(module_name))
                .display()
                .to_string(),
        );
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
