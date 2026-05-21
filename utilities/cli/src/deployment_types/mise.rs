use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use edge_toolkit::input::ClusterInput;
use toml::{Table, Value};

use crate::{absolute_from, cluster_module_names, module_registry, relative_path_from, resolve_module_paths};

pub fn generate_mise_deployment(cluster: &ClusterInput, output_dir: &Path) -> Result<()> {
    let output_path = output_dir.join("mise.toml");
    let workspace_root =
        std::env::current_dir().with_context(|| "Failed to resolve current working directory for mise tasks")?;
    let output_abs = absolute_from(&workspace_root, output_dir);
    let ws_server_dir = workspace_root.join("services/ws-server");
    let workspace_rel = relative_path_from(&output_abs, &workspace_root).display().to_string();
    let openobserve_env_file_rel = "config/o2.env";
    let module_names = cluster_module_names(cluster);
    let module_paths = scenario_module_paths(&ws_server_dir, &module_names)?;
    let module_paths_lines = module_paths
        .iter()
        .map(|p| format!("  {p}"))
        .collect::<Vec<_>>()
        .join(",\\\n");
    let ws_server_run = format!("export MODULES_PATHS=\"\\\n{module_paths_lines}\"\ncargo run\n");
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
            Some(mise_depends(["openobserve-ready"])),
            Some(mise_env()),
        )),
    );
    tasks.insert(
        "openobserve-ready".to_string(),
        Value::Table(mise_task(
            None,
            Some("Wait for OpenObserve to accept connections"),
            None,
            Some("waitup http://127.0.0.1:5080/healthz"),
            None,
            None,
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

pub fn scenario_module_paths(ws_server_dir: &Path, module_names: &[String]) -> Result<Vec<String>> {
    let project_root = edge_toolkit::config::get_project_root();
    let mut paths = vec![
        relative_path_from(ws_server_dir, &project_root.join("services/ws-server/static"))
            .display()
            .to_string(),
        relative_path_from(ws_server_dir, &project_root.join("services/ws-wasm-agent"))
            .display()
            .to_string(),
    ];
    let registry = module_registry(&project_root, ws_server_dir);
    paths.extend(resolve_module_paths(&registry, module_names, |entry| {
        entry.mise_path.clone()
    })?);
    Ok(paths)
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
