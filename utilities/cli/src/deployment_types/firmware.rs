use std::path::Path;

use edge_toolkit::ports::Services;
use et_path::{absolute_from, relative_path_from};
use fs_err as fs;
use toml::{Table, Value};

use super::mise::{mise_task, registry_and_modules, wrap_module_paths};
use crate::error::CliError;
use crate::input::{Agent, ArtifactSource, Board, ClusterInput, Delivery, OutputType};
use crate::placement::agent_site;
use crate::{
    ModuleSource, RunnerInstance, ScenarioModules, cluster_module_names, resolve_cluster_modules, scenario_runners,
};

/// The task that builds the firmware with the scenario's modules in it.
const BUILD_TASK: &str = "firmware-build";
/// The task that loads the firmware onto the board.
pub(crate) const FLASH_TASK: &str = "firmware-flash";
/// The task that serves the same modules from the host, and fetches each, with no board needed.
pub(crate) const HOST_TEST_TASK: &str = "firmware-host-test";
/// Where npm packages too big for the board are fetched from instead: a CDN mirroring the npm registry by package path.
pub(crate) const ELSEWHERE_URL: &str = "https://cdn.jsdelivr.net/npm";
/// Why a published scenario is refused a board.
const PUBLISHED: &str = "uses published artifacts, since the firmware is built from this repository";

/// Refuse a scenario `board` cannot run as a `firmware-*` format, the whole scenario on the one board.
///
/// A second machine is refused: a scenario spanning several, the board among them, names it as a machine's `device:`
/// instead, which [`validate_devices`] checks. A published deployment is refused because the firmware is built from
/// this repository, and the agents are refused what [`validate_board_agents`] refuses them. Checked before anything is
/// written, as every format's refusals are.
pub(crate) fn validate_firmware(cluster: &ClusterInput, board: Board) -> Result<(), CliError> {
    if !cluster.machines.is_empty() {
        return refused(
            board,
            "spans several machines; name the board as a machine's device instead",
        );
    }
    if matches!(cluster.artifact_source, ArtifactSource::Published) {
        return refused(board, PUBLISHED);
    }
    validate_board_agents(board, cluster.agents.iter())
}

/// Refuse a scenario whose machines name a device that `output_type` cannot deliver, or that the device cannot run.
///
/// A board that is the server runs the hub and the runners its firmware hosts beside it; a board on any other machine
/// runs only the runners placed on it, reaching the server's hub across the network, as every machine running its share
/// as the format's tasks does. Only the formats built from `mise.toml` tasks can deliver that, since compose and k3s
/// ship container images a board cannot run.
pub(crate) fn validate_devices(cluster: &ClusterInput, output_type: OutputType) -> Result<(), CliError> {
    let Some(named) = cluster.machines.iter().find(|machine| !machine.device.is_unspecified()) else {
        return Ok(());
    };
    if !output_type.runs_mise_tasks() {
        return Err(CliError::DeviceFormatUnsupported {
            machine: named.name.clone(),
            output_type: output_type.name().to_owned(),
        });
    }
    for (index, machine) in cluster.machines.iter().enumerate() {
        let Delivery::Firmware(board) = machine.device.delivery() else {
            continue;
        };
        if matches!(cluster.artifact_source, ArtifactSource::Published) {
            return refused(board, PUBLISHED);
        }
        if !cluster.services.hub.machines.is_empty() {
            return refused(
                board,
                "runs a hub on another machine too, since the board has no backplane",
            );
        }
        let on_board = || {
            cluster
                .agents
                .iter()
                .filter(|agent| agent_site(cluster, &agent.name).machine_names().contains(&machine.name))
        };
        // A board that is not the hub's machine runs nothing but its runners, so one with none would run nothing.
        if index > 0 && !on_board().any(|agent| agent.runner.is_some()) {
            return Err(CliError::BoardWithoutRunners {
                machine: machine.name.clone(),
                board: board.spec().title,
            });
        }
        validate_board_agents(board, on_board())?;
    }
    Ok(())
}

/// Refuse what the board cannot run, as `board`'s refusal for `reason`.
const fn refused(board: Board, reason: &'static str) -> Result<(), CliError> {
    Err(CliError::BoardCannotRun {
        board: board.spec().title,
        reason,
    })
}

/// Refuse a runner among `agents` that `board` cannot run beside its hub.
///
/// A runner is refused unless the board's firmware runs its kind; whether the guest itself fits, the firmware's build
/// decides. Limits and schedules are refused too, since the firmware has no way to hold a guest to either: it starts
/// what it runs once, at boot.
fn validate_board_agents<'cluster, A>(board: Board, agents: A) -> Result<(), CliError>
where
    A: Iterator<Item = &'cluster Agent>,
{
    let spec = board.spec();
    for agent in agents {
        let Some(runner) = agent.runner.as_deref().map(str::trim).filter(|kind| !kind.is_empty()) else {
            continue;
        };
        if spec.runners.is_empty() {
            return refused(board, "starts a runner, since the board has no memory to run one in");
        }
        if !agent.limits.is_unset() {
            return refused(
                board,
                "sets limits on a runner, since a board has no way to hold one to them",
            );
        }
        if !agent.schedule.is_unset() {
            return refused(
                board,
                "schedules a runner, since a board starts what it runs once, at boot",
            );
        }
        if !spec.runners.contains(&runner) {
            return Err(CliError::BoardRunnerUnsupported {
                board: spec.title,
                agent: agent.name.clone(),
                runner: runner.to_owned(),
                supported: spec.runners.join(", "),
            });
        }
    }
    Ok(())
}

/// Write the `mise.firmware-<board>.toml` that builds the scenario's hub as `board` firmware and loads it onto one.
///
/// Selected with `MISE_ENV=firmware-<board>`, so it sits beside a `mise.toml` without either loading the other's tasks
/// unasked. What the tasks build is [`firmware_tasks`]'s to say.
pub(crate) fn generate_firmware_deployment(
    cluster: &ClusterInput,
    output_dir: &Path,
    board: Board,
) -> Result<(), CliError> {
    validate_firmware(cluster, board)?;
    let guests = scenario_runners(cluster)?;
    let tasks = firmware_tasks(cluster, output_dir, board, &guests, &Build::Whole)?;
    let mut root = Table::new();
    let _previous: Option<Value> = root.insert("tasks".to_owned(), Value::Table(tasks));
    fs::write(
        output_dir.join(board.spec().output_type.output_file_name()),
        toml::to_string(&Value::Table(root))?,
    )?;
    Ok(())
}

/// The tasks of every board among the machines, each suffixed with its machine's name; none when there is no board.
///
/// The server's board runs the hub, and any other board the runners placed on it, reaching the server's hub.
pub(crate) fn board_firmware_tasks(cluster: &ClusterInput, output_dir: &Path) -> Result<Table, CliError> {
    let mut tasks = Table::new();
    let Some(server) = cluster.machines.first() else {
        return Ok(tasks);
    };
    let runners = scenario_runners(cluster)?;
    for machine in &cluster.machines {
        let Delivery::Firmware(board) = machine.device.delivery() else {
            continue;
        };
        let guests = runners
            .iter()
            .filter(|runner| runner.site.machine_names().contains(&machine.name))
            .cloned()
            .collect::<Vec<_>>();
        let build = if machine.name == server.name {
            Build::Hub { machine: &machine.name }
        } else {
            Build::Runners {
                machine: &machine.name,
                hub_address: format!("{}:{}", server.address, Services::InsecureWebSocketServer.port()),
            }
        };
        tasks.extend(firmware_tasks(cluster, output_dir, board, &guests, &build)?);
    }
    Ok(tasks)
}

/// What one board's firmware is built as.
enum Build<'machine> {
    /// The whole scenario on the one board, as a `firmware-*` format puts it.
    Whole,
    /// The hub, and the runners beside it, on `machine`, the server of a scenario spanning several.
    Hub { machine: &'machine str },
    /// The runners on `machine`, reaching the hub at `hub_address`.
    Runners {
        machine: &'machine str,
        hub_address: String,
    },
}

impl Build<'_> {
    /// What ends the name of each of the build's tasks: nothing for a whole scenario, else `-<machine>`.
    fn suffix(&self) -> String {
        match self {
            Self::Whole => String::default(),
            Self::Hub { machine } | Self::Runners { machine, .. } => format!("-{machine}"),
        }
    }

    /// Where the build's image lands, `image` as the board's spec gives it.
    ///
    /// A machine's board builds under a target directory of its own, so that two boards' builds do not overwrite each
    /// other's image; a whole scenario's lands where the spec says.
    fn image(&self, image: &str) -> String {
        match self {
            Self::Whole => image.to_owned(),
            Self::Hub { machine } | Self::Runners { machine, .. } => {
                image.replacen("target/", &format!("{}/", target_dir(machine)), 1)
            }
        }
    }
}

/// The target directory a machine's board is built in.
fn target_dir(machine: &str) -> String {
    format!("target/firmware/{machine}")
}

/// The task that loads `machine`'s board, which is what that machine's share of a scenario amounts to.
pub(crate) fn flash_task(machine: &str) -> String {
    format!("{FLASH_TASK}-{machine}")
}

/// The tasks that build `board`'s firmware as `build` says, load it onto the board, and serve it from this host.
///
/// Only firmware that runs the hub is served from this host: one of runners alone serves nothing to fetch.
///
/// The hub serves the modules the scenario names, and what they depend on, from flash: each module from this
/// repository or the scenario's own `module_paths:` is built into the firmware, and each npm package a mise tool
/// stages is redirected to a CDN instead, since onnxruntime-web alone is bigger than a board's flash. Every module is
/// built in, whichever machine runs it, since a runner elsewhere fetches its module from this hub. A board of runners
/// alone builds in only its runners' modules, and the address of the hub they reach. Each of `guests` is a runner the
/// firmware starts, on its module's component.
fn firmware_tasks(
    cluster: &ClusterInput,
    output_dir: &Path,
    board: Board,
    guests: &[RunnerInstance],
    build: &Build<'_>,
) -> Result<Table, CliError> {
    let board = board.spec();
    let suffix = build.suffix();
    let workspace_root = edge_toolkit::config::get_project_root();
    let output_abs = absolute_from(&workspace_root, output_dir);
    let workspace_rel = relative_path_from(&output_abs, &workspace_root);
    let (modules_paths, env) = module_env(cluster, guests, build, &workspace_root)?;
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
    let role = if matches!(build, Build::Runners { .. }) {
        "running the runners of"
    } else {
        "serving"
    };
    let build_task = module_task(
        &format!("Build the {} firmware {role} {}", board.title, cluster.cluster_name),
        &format!("mise -E rust run {}", board.build_task),
    );

    let build_name = format!("{BUILD_TASK}{suffix}");
    let mut depends = Table::new();
    let _previous: Option<Value> = depends.insert(
        "depends".to_owned(),
        Value::Array(vec![Value::String(build_name.clone())]),
    );
    let flash_run = format!("{} {}", board.load_command, build.image(board.image));
    let flash_description = format!("Load the {} firmware onto the board, and start it", board.title);
    let flash = mise_task(
        None,
        Some(&flash_description),
        Some(&workspace_rel),
        Some(&flash_run),
        Some(depends),
        None,
    );

    let mut tasks = Table::new();
    let _previous: Option<Value> = tasks.insert(build_name, Value::Table(build_task));
    let _previous: Option<Value> = tasks.insert(format!("{FLASH_TASK}{suffix}"), Value::Table(flash));
    // A board of runners alone serves nothing, so there is nothing of it to fetch from this host.
    if !matches!(build, Build::Runners { .. }) {
        let host_test = module_task(
            &format!(
                "Serve {}'s modules from this host as the board would, and fetch each",
                cluster.cluster_name
            ),
            &format!("mise run cargo-test-pkg {}", board.package),
        );
        let _previous: Option<Value> = tasks.insert(format!("{HOST_TEST_TASK}{suffix}"), Value::Table(host_test));
    }
    Ok(tasks)
}

/// The `MODULES_PATHS` assignments naming the modules built in, and the rest of the build's environment.
///
/// That is the other `MODULES_*` variables, and `RUNNER_GUESTS`: a `name=package` pair for each of `guests`, naming
/// the module built in whose component the firmware compiles for it. Set even when empty, so a build never picks
/// runners up from the shell it was started in. A machine's board also builds into a target directory of its own, and a
/// board of runners alone is told the `HUB_ADDRESS` they reach.
#[expect(
    clippy::single_call_fn,
    reason = "the module-resolving step of firmware_tasks, kept apart from assembling the tasks"
)]
fn module_env(
    cluster: &ClusterInput,
    guests: &[RunnerInstance],
    build: &Build<'_>,
    workspace_root: &Path,
) -> Result<(String, Table), CliError> {
    let ws_server_dir = workspace_root.join("services/ws-server");
    // A hub serves the page whether or not the board runs agents of its own, since a browser can still join it. A
    // board of runners alone serves nothing, so builds in only what its runners' guests are compiled from.
    let (module_names, serves_a_page) = match build {
        Build::Whole | Build::Hub { .. } => (cluster_module_names(cluster), true),
        Build::Runners { machine, .. } => (machine_modules(cluster, machine), false),
    };
    let scenario =
        ScenarioModules::new(&ws_server_dir, &module_names, serves_a_page).with_module_paths(&cluster.module_paths);
    let guests = guests
        .iter()
        .map(|runner| format!("{}={}", runner.name, runner.module))
        .collect::<Vec<_>>();

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
    let root = if serves_a_page {
        super::hub_root_module(&ws_server_dir)
    } else {
        String::default()
    };
    set("MODULES_ROOT", root);
    set("MODULES_ELSEWHERE", elsewhere.join(", "));
    set("MODULES_ELSEWHERE_URL", ELSEWHERE_URL.to_owned());
    set("RUNNER_GUESTS", guests.join(", "));
    match build {
        Build::Whole => {}
        Build::Hub { machine } => set("CARGO_TARGET_DIR", target_dir(machine)),
        Build::Runners { machine, hub_address } => {
            set("CARGO_TARGET_DIR", target_dir(machine));
            set("HUB_ADDRESS", hub_address.clone());
        }
    }
    Ok((wrap_module_paths(&embedded), env))
}

/// The modules of the runners placed on `machine`, by the names the scenario gives them.
fn machine_modules(cluster: &ClusterInput, machine: &str) -> Vec<String> {
    cluster
        .agents
        .iter()
        .filter(|agent| agent.runner.is_some())
        .filter(|agent| {
            agent_site(cluster, &agent.name)
                .machine_names()
                .iter()
                .any(|placed| placed == machine)
        })
        .flat_map(|agent| {
            agent
                .resources
                .iter()
                .map(|resource| resource.resource_type.trim().to_owned())
        })
        .collect()
}
