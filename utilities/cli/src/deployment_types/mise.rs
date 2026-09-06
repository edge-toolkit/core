use std::path::Path;

use edge_toolkit::input::ClusterInput;
use et_path::{absolute_from, relative_path_from};
use fs_err as fs;
use toml::{Table, Value};

use crate::error::CliError;
use crate::{cluster_module_names, module_registry, resolve_module_paths};

pub fn generate_mise_deployment(cluster: &ClusterInput, output_dir: &Path, password: &str) -> Result<(), CliError> {
    let output_path = output_dir.join("mise.toml");
    let workspace_root = edge_toolkit::config::get_project_root();
    let output_abs = absolute_from(&workspace_root, output_dir);
    let ws_server_dir = workspace_root.join("services/ws-server");
    let workspace_rel = relative_path_from(&output_abs, &workspace_root);
    let openobserve_env_file_rel = "config/o2.env";
    // The image and the credential override are lifted into shell variables, not folded with continuations.
    // Inlining both would put the `docker run` past the editorconfig line length once the generated password is
    // long enough, and a wrapped copy is what once silently dropped `-it`; a variable keeps the command one
    // statement whatever the password turns out to be. `-e` comes after `--env-file` so the scenario password
    // wins over the repo-wide one the env file carries.
    let openobserve_run = format!(
        concat!(
            "image=openobserve/openobserve:v0.91.5\n",
            "credential=ZO_ROOT_USER_PASSWORD={}\n",
            "docker run --rm --name openobserve -p 5080:5080 --env-file {} -e \"$credential\" \"$image\"\n",
        ),
        password, openobserve_env_file_rel
    );
    let module_names = cluster_module_names(cluster);
    let module_paths = scenario_module_paths(&ws_server_dir, &module_names)?;
    let module_paths_lines = wrap_module_paths(&module_paths);
    let ws_server_run = format!("export MODULES_PATHS=\"\\\n{module_paths_lines}\"\ncargo run\n");
    let ws_server_rel = relative_path_from(&output_abs, &ws_server_dir);

    let mut root = Table::new();
    let mut tasks = Table::new();

    let _previous: Option<Value> = tasks.insert(
        "openobserve".to_string(),
        Value::Table(mise_task(
            Some("o2"),
            None,
            Some(&workspace_rel),
            Some(&openobserve_run),
            None,
            None,
        )),
    );
    let _previous: Option<Value> = tasks.insert(
        "ws-server".to_string(),
        Value::Table(mise_task(
            None,
            Some("Run the WebSocket server"),
            Some(&ws_server_rel),
            Some(&ws_server_run),
            None,
            Some(mise_env(password)),
        )),
    );
    let _previous: Option<Value> = tasks.insert(
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
    let _previous: Option<Value> = tasks.insert(
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

    let _previous: Option<Value> = root.insert("tasks".to_string(), Value::Table(tasks));

    let mut tools = Table::new();
    let _previous: Option<Value> = tools.insert("cargo:open".to_string(), Value::String("latest".to_string()));
    let _previous: Option<Value> = root.insert("tools".to_string(), Value::Table(tools));

    let content = toml::to_string(&Value::Table(root))?;
    fs::write(&output_path, content)?;

    Ok(())
}

pub fn scenario_module_paths(ws_server_dir: &Path, module_names: &[String]) -> Result<Vec<String>, CliError> {
    let project_root = edge_toolkit::config::get_project_root();
    let mut paths = vec![
        relative_path_from(ws_server_dir, &project_root.join("services/ws-server/static")),
        relative_path_from(ws_server_dir, &project_root.join("services/ws-wasm-agent")),
    ];
    let registry = module_registry(&project_root, ws_server_dir);
    paths.extend(resolve_module_paths(&registry, module_names, |entry| {
        entry.mise_path.clone()
    })?);
    Ok(paths)
}

// Pack `paths` into `,\`-continued lines within the editorconfig line length via textwrap first-fit bin-packing.
// Each path is one atomic fragment (some hold spaces, e.g. `$(mise where ...)`, so they must never be split);
// paths sharing a line are joined by `, `, and the shell `\` line-continuations plus the consumer's per-segment
// trim make the folded value exactly the comma-separated path list. Every emitted line is `  <paths>` plus a
// trailing `,\`, so the fit budget is that line length minus the 2-space indent and the 2-char continuation.
fn wrap_module_paths(paths: &[String]) -> String {
    #[derive(Debug)]
    struct PathFragment<'path> {
        path: &'path str,
        width: f64,
    }
    impl textwrap::core::Fragment for PathFragment<'_> {
        fn width(&self) -> f64 {
            self.width
        }
        fn whitespace_width(&self) -> f64 {
            2.0 // the ", " joining two paths on one line
        }
        fn penalty_width(&self) -> f64 {
            0.0
        }
    }

    const LINE_WIDTH: f64 = 116.0;
    let fragments: Vec<PathFragment> = paths
        .iter()
        .map(|path| PathFragment {
            path,
            width: f64::from(u32::try_from(path.chars().count()).unwrap_or(u32::MAX)),
        })
        .collect();

    textwrap::wrap_algorithms::wrap_first_fit(&fragments, &[LINE_WIDTH])
        .iter()
        .map(|group| {
            let joined = group
                .iter()
                .map(|fragment| fragment.path)
                .collect::<Vec<_>>()
                .join(", ");
            format!("  {joined}")
        })
        .collect::<Vec<_>>()
        .join(",\\\n")
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
        let _previous: Option<Value> = task.insert("alias".to_string(), Value::String(alias.to_string()));
    }
    if let Some(description) = description {
        let _previous: Option<Value> = task.insert("description".to_string(), Value::String(description.to_string()));
    }
    if let Some(dir) = dir {
        let _previous: Option<Value> = task.insert("dir".to_string(), Value::String(dir.to_string()));
    }
    if let Some(run) = run {
        let _previous: Option<Value> = task.insert("run".to_string(), Value::String(run.to_string()));
    }
    if let Some(extra) = extra {
        for (key, value) in extra {
            let _previous: Option<Value> = task.insert(key, value);
        }
    }
    if let Some(env) = env {
        let _previous: Option<Value> = task.insert("env".to_string(), Value::Table(env));
    }
    task
}

fn mise_env(password: &str) -> Table {
    let mut env = Table::new();
    let _previous: Option<Value> = env.insert("OTLP_AUTH_PASSWORD".to_string(), Value::String(password.to_string()));
    let _previous: Option<Value> = env.insert(
        "OTLP_AUTH_USERNAME".to_string(),
        Value::String("root@example.com".to_string()),
    );
    env
}

fn mise_depends<const N: usize>(depends: [&str; N]) -> Table {
    let mut extra = Table::new();
    let _previous: Option<Value> = extra.insert(
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
