use std::fmt::Write as _;
use std::path::Path;

use edge_toolkit::input::ClusterInput;
use et_path::{absolute_from, relative_path_from};
use fs_err as fs;
use toml::{Table, Value};

use crate::error::CliError;
use crate::{
    RunnerInstance, SECRET_PRAGMA, cluster_module_names, hub_ws_url, module_registry, resolve_cluster_runners,
    resolve_module_paths, runner_crate,
};

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
            "credential=ZO_ROOT_USER_PASSWORD={} {}\n",
            "docker run --rm --name openobserve -p 127.0.0.1:5080:5080 --env-file {} -e \"$credential\" ",
            "\"$image\"\n",
        ),
        password, SECRET_PRAGMA, openobserve_env_file_rel
    );
    let module_names = cluster_module_names(cluster);
    let module_paths = scenario_module_paths(&ws_server_dir, &module_names)?;
    let module_paths_lines = wrap_module_paths(&module_paths);
    let ws_server_run = format!("{module_paths_lines}export MODULES_PATHS\ncargo run\n");
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
    // Each runner agent becomes its own task, and `generated-scenario` depends on all of them.
    // mise runs `depends` concurrently, which is what a cluster wants -- the hub and every runner are
    // long-running peers, not a pipeline -- but it also means a runner starts before the hub is listening. Each
    // runner body therefore waits for the hub's own health endpoint first; see `runner_run_body`.
    let runners = resolve_cluster_runners(&module_registry(&workspace_root, &ws_server_dir), cluster)?;
    for runner in &runners {
        let _previous: Option<Value> = tasks.insert(
            runner.name.clone(),
            Value::Table(mise_task(
                None,
                Some(&format!("Run {} in the {} runner", runner.module, runner.runner)),
                Some(&workspace_rel),
                Some(&runner_run_body(runner)),
                None,
                Some(runner_env(runner)),
            )),
        );
    }

    let mut scenario_depends = vec!["openobserve".to_string(), "ws-server".to_string()];
    scenario_depends.extend(runners.iter().map(|runner| runner.name.clone()));
    let _previous: Option<Value> = tasks.insert(
        "generated-scenario".to_string(),
        Value::Table(mise_task(
            None,
            Some(&format!("Run generated scenario for {}", cluster.cluster_name)),
            None,
            None,
            Some(mise_depends(&scenario_depends)),
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
    // No extra tools for the runner tasks: `runner_run_body` calls only `cargo`, which the hub task needs anyway.
    let _previous: Option<Value> = root.insert("tools".to_string(), Value::Table(tools));

    // The pragma is inserted after serialization because the toml crate has no way to emit a comment.
    // Everything else here goes through the `Value` tree, but a comment is not part of the data model, so the
    // one credential line is rewritten in the finished document instead.
    //
    // On its own line rather than trailing the value, because taplo aligns trailing comments across a table
    // while this emits a single space. Trailing, the two disagree permanently: `taplo-fmt` pads the generated
    // file and the next `regen-verification` unpads it, so `verification-check` reports drift either way round.
    let credential = format!("OTLP_AUTH_PASSWORD = \"{password}\"");
    let content = toml::to_string(&Value::Table(root))?.replace(&credential, &format!("{SECRET_PRAGMA}\n{credential}"));
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

// Pack `paths` into a run of `MODULES_PATHS=` assignments within the editorconfig line length, via textwrap
// first-fit bin-packing. Each path is one atomic fragment (some hold spaces, e.g. `$(mise where ...)`, so they
// must never be split) and paths sharing a line are joined by `, `. Each line after the first appends to the
// variable instead of continuing it with a trailing `\`, so the assembled value is the same comma-separated list
// the consumer's per-segment trim expects while the body stays free of line-continuations -- a `\` that picks up
// trailing whitespace silently ends the statement early, and the repo bans the form outside README files.
// The two fit budgets are the line length minus each form's fixed prefix and its closing quote: `MODULES_PATHS="`
// for the opening line, `MODULES_PATHS="$MODULES_PATHS, ` for every later one.
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

    const FIRST_LINE_WIDTH: f64 = 104.0;
    const APPEND_LINE_WIDTH: f64 = 88.0;
    let fragments: Vec<PathFragment> = paths
        .iter()
        .map(|path| PathFragment {
            path,
            width: f64::from(u32::try_from(path.chars().count()).unwrap_or(u32::MAX)),
        })
        .collect();

    let groups = textwrap::wrap_algorithms::wrap_first_fit(&fragments, &[FIRST_LINE_WIDTH, APPEND_LINE_WIDTH]);
    let mut out = String::default();
    for (index, group) in groups.iter().enumerate() {
        let joined = group
            .iter()
            .map(|fragment| fragment.path)
            .collect::<Vec<_>>()
            .join(", ");
        let _write_result = if index == 0 {
            writeln!(out, "MODULES_PATHS=\"{joined}\"")
        } else {
            writeln!(out, "MODULES_PATHS=\"$MODULES_PATHS, {joined}\"")
        };
    }
    out
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

/// Build a task's `depends` array, sorted.
///
/// Sorted because `config/taplo.toml` sets `reorder_arrays = true`, so `taplo-fmt` sorts this array in the
/// committed file. Emitting it in the order the tasks happen to be assembled leaves the two permanently at odds:
/// the formatter sorts the generated file and the next `regen-verification` unsorts it, so `verification-check`
/// reports drift whichever ran last. The order carries no meaning to mise either -- `depends` is a set of
/// prerequisites it starts together, not a sequence.
fn mise_depends(depends: &[String]) -> Table {
    let mut sorted = depends.to_vec();
    sorted.sort_unstable();
    let mut extra = Table::new();
    let _previous: Option<Value> = extra.insert(
        "depends".to_string(),
        Value::Array(sorted.into_iter().map(Value::String).collect()),
    );
    extra
}

/// Render a runner task's body: wait for the hub to serve this module, then run the runner.
///
/// The wait is the whole reason this is two commands rather than a bare `cargo run`. `depends` starts the hub and
/// the runners together, and a runner that wins the race dies immediately -- it resolves the module by fetching
/// `/modules/<name>/package.json` over HTTP, which fails outright rather than retrying, so without this the
/// scenario is a coin toss.
///
/// `et-cli wait-for-module` does the waiting rather than a shell poll loop, and that choice is load-bearing. A
/// loop needs an HTTP client, and a generated deployment cannot guarantee one: an earlier version polled with
/// `xh`, declared in this file's own `[tools]` -- but `task.run_auto_install` is off, so nothing installed it,
/// and it is pinned only in the maintainer-only env. The task read as correct and silently spun out its whole
/// timeout wherever that tool was absent, CI included. Deferring to `et-cli` leaves the body needing no tool
/// beyond the `cargo` it already uses for the runner, and retires the shell-portability question with the loop
/// (`SECONDS` is a bashism, `sleep` another tool).
fn runner_run_body(runner: &RunnerInstance) -> String {
    let crate_name = runner_crate(&runner.runner);
    let mut body = String::default();
    let _write_result = writeln!(
        body,
        "cargo run --quiet -p et-cli -- wait-for-module --module {}",
        runner.module
    );
    let _write_result = writeln!(body, "cargo run --quiet -p {crate_name}");
    body
}

/// The `RUNNER_*`/`WS_*` environment a runner task needs.
///
/// `WS_SERVER_URL` is spelled out rather than left to the runner's default so the generated task keeps working
/// if that default ever moves, and it is also what the runner derives its HTTP base from.
fn runner_env(runner: &RunnerInstance) -> Table {
    let mut env = Table::new();
    let _previous: Option<Value> = env.insert("RUNNER_MODULE".to_string(), Value::String(runner.module.clone()));
    let _previous: Option<Value> = env.insert("WS_SERVER_URL".to_string(), Value::String(hub_ws_url()));
    env
}
