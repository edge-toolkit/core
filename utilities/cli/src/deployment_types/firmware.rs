use std::path::Path;

use et_path::{absolute_from, relative_path_from};
use fs_err as fs;
use toml::{Table, Value};

use super::mise::{mise_task, registry_and_modules, wrap_module_paths};
use crate::error::CliError;
use crate::input::{ArtifactSource, Board, ClusterInput};
use crate::{ModuleSource, ScenarioModules, cluster_module_names, resolve_cluster_modules};

/// The task that builds the firmware with the scenario's modules in it.
const BUILD_TASK: &str = "firmware-build";
/// The task that loads the firmware onto the board.
pub(crate) const FLASH_TASK: &str = "firmware-flash";
/// The task that serves the same modules from the host, and fetches each, with no board needed.
pub(crate) const HOST_TEST_TASK: &str = "firmware-host-test";
/// Where npm packages too big for the board are fetched from instead: a CDN mirroring the npm registry by package path.
pub(crate) const ELSEWHERE_URL: &str = "https://cdn.jsdelivr.net/npm";

/// Refuse a scenario `board` cannot run: it holds one hub and the modules built into it, and nothing else.
///
/// A runner needs an OS to run on, a second machine a backplane, and a published deployment modules staged from a
/// release, none of which the firmware has. A second hub needs a second machine, so is refused with it. Checked before
/// anything is written, as every format's refusals are.
pub(crate) fn validate_firmware(cluster: &ClusterInput, board: Board) -> Result<(), CliError> {
    let reason = if cluster.agents.iter().any(|agent| agent.runner.is_some()) {
        "starts a runner"
    } else if !cluster.machines.is_empty() {
        "spans several machines"
    } else if matches!(cluster.artifact_source, ArtifactSource::Published) {
        "uses published artifacts, since the firmware is built from this repository"
    } else {
        return Ok(());
    };
    Err(CliError::BoardCannotRun {
        board: board.spec().title,
        reason,
    })
}

/// Write the `mise.firmware-<board>.toml` that builds the scenario's hub as `board` firmware and loads it onto one.
///
/// The board serves the modules the scenario names, and what they depend on, from flash: each module from this
/// repository or the scenario's own `module_paths:` is built into the firmware, and each npm package a mise tool
/// stages is redirected to a CDN instead, since onnxruntime-web alone is bigger than a board's flash. Selected with
/// `MISE_ENV=firmware-<board>`, so it sits beside a `mise.toml` without either loading the other's tasks unasked.
pub(crate) fn generate_firmware_deployment(
    cluster: &ClusterInput,
    output_dir: &Path,
    board: Board,
) -> Result<(), CliError> {
    validate_firmware(cluster, board)?;
    let board = board.spec();
    let workspace_root = edge_toolkit::config::get_project_root();
    let output_abs = absolute_from(&workspace_root, output_dir);
    let workspace_rel = relative_path_from(&output_abs, &workspace_root);
    let (modules_paths, env) = module_env(cluster, &workspace_root)?;
    // Both run at the repository root, where `$PWD` names the module paths absolutely, as the build needs.
    let module_task = |description: &str, command: &str| {
        let run = format!("{modules_paths}export MODULES_PATHS\n{command}\n");
        mise_task(
            None,
            Some(description),
            Some(&workspace_rel),
            Some(&run),
            None,
            Some(env.clone()),
        )
    };
    let build = module_task(
        &format!("Build the {} firmware serving {}", board.title, cluster.cluster_name),
        &format!("mise -E rust run {}", board.build_task),
    );
    let host_test = module_task(
        &format!(
            "Serve {}'s modules from this host as the board would, and fetch each",
            cluster.cluster_name
        ),
        &format!("mise run cargo-test-pkg {}", board.package),
    );

    let mut depends = Table::new();
    let _previous: Option<Value> = depends.insert(
        "depends".to_owned(),
        Value::Array(vec![Value::String(BUILD_TASK.to_owned())]),
    );
    let flash_run = format!("{} {}", board.load_command, board.image);
    let flash_description = format!("Load the firmware onto a {}, and start it", board.title);
    let flash = mise_task(
        None,
        Some(&flash_description),
        Some(&workspace_rel),
        Some(&flash_run),
        Some(depends),
        None,
    );

    let mut tasks = Table::new();
    let _previous: Option<Value> = tasks.insert(BUILD_TASK.to_owned(), Value::Table(build));
    let _previous: Option<Value> = tasks.insert(FLASH_TASK.to_owned(), Value::Table(flash));
    let _previous: Option<Value> = tasks.insert(HOST_TEST_TASK.to_owned(), Value::Table(host_test));
    let mut root = Table::new();
    let _previous: Option<Value> = root.insert("tasks".to_owned(), Value::Table(tasks));
    fs::write(
        output_dir.join(board.output_type.output_file_name()),
        toml::to_string(&Value::Table(root))?,
    )?;
    Ok(())
}

/// The `MODULES_PATHS` assignments naming the modules built in, and the rest of the `MODULES_*` environment.
#[expect(
    clippy::single_call_fn,
    reason = "the module-resolving step of generate_firmware_deployment, kept apart from writing the file"
)]
fn module_env(cluster: &ClusterInput, workspace_root: &Path) -> Result<(String, Table), CliError> {
    let ws_server_dir = workspace_root.join("services/ws-server");
    let module_names = cluster_module_names(cluster);
    // With no runner allowed, every agent is one the page runs, so the board always serves the page.
    let scenario = ScenarioModules::new(&ws_server_dir, &module_names, true).with_module_paths(&cluster.module_paths);

    let (registry, modules) = registry_and_modules(&scenario);
    let mut embedded = Vec::new();
    let mut elsewhere = Vec::new();
    for entry in resolve_cluster_modules(&registry, &modules)? {
        match entry.source {
            ModuleSource::Repo(path) => embedded.push(format!("$PWD/{path}")),
            ModuleSource::Scenario { dir, .. } => embedded.push(dir.display().to_string()),
            ModuleSource::MiseTool { package, .. } => elsewhere.push(package),
        }
    }
    embedded.sort();
    elsewhere.sort();

    let mut env = Table::new();
    let mut set = |name: &str, value: String| {
        let _previous: Option<Value> = env.insert(name.to_owned(), Value::String(value));
    };
    set("MODULES_ROOT", super::hub_root_module(&ws_server_dir));
    set("MODULES_ELSEWHERE", elsewhere.join(", "));
    set("MODULES_ELSEWHERE_URL", ELSEWHERE_URL.to_owned());
    Ok((wrap_module_paths(&embedded), env))
}
