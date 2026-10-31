use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use edge_toolkit::ports::Services;
use et_path::{absolute_from, relative_path_from};
use fs_err as fs;
use toml::{Table, Value};

use super::hub_sharing::{
    BACKPLANE_IMAGE, BACKPLANE_TASK, DATA_DIR, SHARED_STORAGE_IMAGE, SHARED_STORAGE_TASK, backplane_args,
    shared_hub_env, shared_storage_env, shared_storage_start,
};
use super::process_compose::PROCESS_COMPOSE_TOOL;
use crate::error::CliError;
use crate::input::{ArtifactSource, ClusterInput};
use crate::placement::{hubs, shares_hubs};
use crate::{
    COLLECTOR_SETTINGS, COLLECTOR_USERNAME, MODULE_SCOPE, ModuleRegistryEntry, ModuleSource, RunnerInstance,
    cluster_module_names, module_registry, resolve_cluster_modules, resolve_module_paths, runner_crate,
    scenario_runners,
};

/// Crate whose binary serves the hub, which a published deployment installs in place of building it.
const HUB_CRATE: &str = "et-ws-server";

/// Prefix that hands a module path to the hub as a package to resolve rather than a directory to read.
///
/// A staged package's directory is not knowable when a deployment is generated -- mise's npm backend picks one of
/// several layouts depending on backend and platform -- so the name is passed through and the hub resolves it, using
/// the same code its own default search paths do. The deployment therefore needs nothing installed to work out where a
/// module landed, which is what keeps this project's generator out of it.
const STAGED_PREFIX: &str = "npm:";

/// npm config the generated deployment points mise at, so `npm:` tools resolve against GitHub Packages.
///
/// A file beside `mise.toml` rather than a setting, because a registry and its credential are npm's own config
/// rather than mise's, and mise reads user-level npm config while deliberately ignoring a project `.npmrc` --
/// `NPM_CONFIG_USERCONFIG` is the one handle that reaches it. The credential is written as a `${GITHUB_TOKEN}`
/// reference that npm expands when it reads the file, so the deployment carries no secret.
const NPMRC_FILE: &str = "npmrc";

/// Directory, beside `mise.toml`, a published deployment's hub keeps agent storage in.
pub(crate) const STORAGE_DIR: &str = "storage";

/// The mise task that runs the collector, which the other deployment formats and runner-name checks refer to.
pub(crate) const COLLECTOR_TASK: &str = "openobserve";

/// The mise task that runs the hub.
pub(crate) const HUB_TASK: &str = "ws-server";

/// The mise task that opens the collector's UI in a browser.
pub(crate) const OPENER_TASK: &str = "open-o2";

/// Version every `cargo:` tool the generated deployment declares is requested at.
const LATEST: &str = "latest";

/// The page served at `/`, which only a cluster a browser opens has any use for.
///
/// `static` names the runtimes its page imports at boot, so a deployment that serves the page without it serves one
/// whose first import 404s -- and one that serves neither is a headless cluster that was never going to load either.
const PAGE_MODULE: &str = "static";

/// The mise task that starts everything one machine of a multi-machine cluster runs.
pub(crate) fn machine_task(machine: &str) -> String {
    format!("machine-{machine}")
}

/// Name the collector task gives its container, so anything stopping the task can stop the container by it.
///
/// Scoped to the scenario so a stop can only ever reach this scenario's collector: a bare `openobserve` is also the
/// name this repository's own `o2` task runs its collector under, and any other scenario's on the same Docker daemon.
pub(crate) fn collector_container(cluster_name: &str) -> String {
    container(COLLECTOR_TASK, cluster_name)
}

/// Name the task `task` gives the container it runs, scoped to the scenario as [`collector_container`] explains.
pub(crate) fn container(task: &str, cluster_name: &str) -> String {
    format!("{task}-{cluster_name}")
}

/// The tasks the server runs besides its runners: the collector, its hub, and what the hubs share when there are more.
fn server_tasks(cluster: &ClusterInput) -> Vec<String> {
    let mut tasks = vec![COLLECTOR_TASK.to_string(), HUB_TASK.to_string()];
    if shares_hubs(cluster) {
        tasks.extend([BACKPLANE_TASK.to_string(), SHARED_STORAGE_TASK.to_string()]);
    }
    tasks
}

/// The task that runs one runner agent, from the checkout root for a local scenario.
#[expect(clippy::single_call_fn, reason = "the per-runner step of generate_mise_deployment")]
fn runner_task(runner: &RunnerInstance, workspace_rel: &str, artifacts: ArtifactSource) -> Table {
    mise_task(
        None,
        Some(&format!("Run {} in the {} runner", runner.module, runner.runner)),
        workspace_dir(workspace_rel, artifacts),
        Some(&runner_run_body(runner, artifacts)),
        None,
        Some(runner_env(runner)),
    )
}

/// Add the tasks a person runs: `generated-scenario`, which starts every role together, and the UI opener.
///
/// `generated-scenario` starts the server's hub alone, never another machine's: every hub listens on the same ports, so
/// on one host a second could not start, and each runner reaches the one there is at `localhost` anyway.
#[expect(clippy::single_call_fn, reason = "the entry-point step of generate_mise_deployment")]
fn insert_entry_tasks(tasks: &mut Table, cluster: &ClusterInput, runners: &[RunnerInstance]) {
    let mut scenario_depends = server_tasks(cluster);
    scenario_depends.extend(runners.iter().map(|runner| runner.name.clone()));
    let description = format!("Run generated scenario for {}", cluster.cluster_name);
    let scenario = mise_task(
        None,
        Some(&description),
        None,
        None,
        Some(mise_depends(&scenario_depends)),
        None,
    );
    let _previous: Option<Value> = tasks.insert("generated-scenario".to_string(), Value::Table(scenario));
    insert_machine_tasks(tasks, cluster, runners);
    let opener = mise_task(
        None,
        Some("Open the OpenObserve UI"),
        None,
        Some("open http://localhost:5080/"),
        None,
        None,
    );
    let _previous: Option<Value> = tasks.insert(OPENER_TASK.to_string(), Value::Table(opener));
}

/// Add one `machine-<name>` task per machine, each starting what that machine runs and nothing else.
///
/// The server's also starts the collector and the hub, and any other machine running a hub starts its own. `generated-
/// scenario` stays alongside them, still starting everything, because the whole cluster on one host remains the
/// quickest way to run it.
#[expect(clippy::single_call_fn, reason = "the per-machine step of insert_entry_tasks")]
fn insert_machine_tasks(tasks: &mut Table, cluster: &ClusterInput, runners: &[RunnerInstance]) {
    let hubs = hubs(cluster);
    for (index, machine) in cluster.machines.iter().enumerate() {
        let mut depends = if index == 0 {
            server_tasks(cluster)
        } else {
            hubs.iter()
                .filter(|hub| hub.site.machine_names().contains(&machine.name))
                .map(|hub| hub.name.clone())
                .collect()
        };
        depends.extend(
            runners
                .iter()
                .filter(|runner| runner.site.machine_names().contains(&machine.name))
                .map(|runner| runner.name.clone()),
        );
        let description = format!("Run what machine {} runs", machine.name);
        let task = mise_task(None, Some(&description), None, None, Some(mise_depends(&depends)), None);
        let _previous: Option<Value> = tasks.insert(machine_task(&machine.name), Value::Table(task));
    }
}

/// Write the `mise.toml` that runs the scenario: the collector, the hub and one task per runner.
///
/// A published scenario also gets the npm config its module packages resolve against. `with_process_compose` declares
/// process-compose among the tools, for a `process-compose.yaml` written alongside that runs these tasks.
pub fn generate_mise_deployment(
    cluster: &ClusterInput,
    output_dir: &Path,
    with_process_compose: bool,
) -> Result<(), CliError> {
    let output_path = output_dir.join("mise.toml");
    let workspace_root = edge_toolkit::config::get_project_root();
    let output_abs = absolute_from(&workspace_root, output_dir);
    let ws_server_dir = workspace_root.join("services/ws-server");
    let workspace_rel = relative_path_from(&output_abs, &workspace_root);
    let module_names = cluster_module_names(cluster);
    let artifacts = cluster.artifact_source;
    let scenario = ScenarioModules::new(&ws_server_dir, &module_names, super::serves_a_page(cluster))
        .with_module_paths(&cluster.module_paths);
    let staged = if matches!(artifacts, ArtifactSource::Published) {
        staged_modules(&scenario)?
    } else {
        Vec::new()
    };

    let mut root = Table::new();
    let mut tasks = service_tasks(cluster, &scenario, &output_abs, &ws_server_dir)?;
    // Each runner agent becomes its own task, and `generated-scenario` depends on all of them. mise runs `depends`
    // concurrently, which is what a cluster wants -- the hub and every runner are long-running peers, not a pipeline --
    // but it also means a runner starts before the hub is listening. Each runner body therefore waits for the hub's own
    // health endpoint first; see `runner_run_body`.
    let runners = scenario_runners(cluster)?;
    for runner in &runners {
        let task = runner_task(runner, &workspace_rel, artifacts);
        let _previous: Option<Value> = tasks.insert(runner.name.clone(), Value::Table(task));
    }

    insert_entry_tasks(&mut tasks, cluster, &runners);

    let _previous: Option<Value> = root.insert("env".to_string(), Value::Table(mise_env(artifacts)));
    let _previous: Option<Value> = root.insert("tasks".to_string(), Value::Table(tasks));
    let tools = mise_tools(&runners, artifacts, &staged, with_process_compose);
    let _previous: Option<Value> = root.insert("tools".to_string(), Value::Table(tools));

    fs::write(&output_path, toml::to_string(&Value::Table(root))?)?;
    if matches!(artifacts, ArtifactSource::Published) {
        fs::write(output_dir.join(NPMRC_FILE), npmrc_contents())?;
    }

    Ok(())
}

/// The tasks every deployment has whatever its agents: the collector and the hub.
#[expect(
    clippy::single_call_fn,
    reason = "the fixed-services step of generate_mise_deployment"
)]
fn service_tasks(
    cluster: &ClusterInput,
    scenario: &ScenarioModules,
    output_abs: &Path,
    ws_server_dir: &Path,
) -> Result<Table, CliError> {
    let artifacts = cluster.artifact_source;
    let ws_server_run = ws_server_run_body(scenario, artifacts, output_abs)?;
    let ws_server_rel = relative_path_from(output_abs, ws_server_dir);
    let mut tasks = Table::new();
    // No working directory: the collector is a container started from values this file carries, so unlike the hub it
    // has nothing to resolve against the repository.
    let openobserve_run = openobserve_run_body(&cluster.cluster_name);
    let _previous: Option<Value> = tasks.insert(
        COLLECTOR_TASK.to_string(),
        Value::Table(mise_task(Some("o2"), None, None, Some(&openobserve_run), None, None)),
    );
    // A published hub is handed absolute paths that `mise where` resolves, so it needs no directory of its own; a local
    // one is given paths relative to the server's source dir and has to start there.
    let shared = shares_hubs(cluster);
    for hub in hubs(cluster) {
        let description = match hub.site.machine_names().as_slice() {
            [machine] if !hub.on_server() => format!("Run the WebSocket server on {machine}"),
            _ => "Run the WebSocket server".to_string(),
        };
        let mut env = ws_server_env(scenario, artifacts, shared);
        if shared {
            for (name, value) in shared_hub_env(&hub.server_host, &hub.server_host) {
                let _previous: Option<Value> = env.insert(name.to_string(), Value::String(value));
            }
        }
        let task = mise_task(
            None,
            Some(&description),
            workspace_dir(&ws_server_rel, artifacts),
            Some(&ws_server_run),
            None,
            Some(env),
        );
        let _previous: Option<Value> = tasks.insert(hub.name, Value::Table(task));
    }
    if shared {
        let backplane = backplane_run_body(&cluster.cluster_name);
        let storage = shared_storage_run_body(&cluster.cluster_name);
        for (name, description, run) in [
            (BACKPLANE_TASK, "Run the NATS server the hubs share", backplane),
            (SHARED_STORAGE_TASK, "Run the object store the hubs share", storage),
        ] {
            let task = mise_task(None, Some(description), None, Some(&run), None, None);
            let _previous: Option<Value> = tasks.insert(name.to_string(), Value::Table(task));
        }
    }
    Ok(tasks)
}

/// The body of the task that starts the NATS container, its client port open to the other machines' hubs.
///
/// The monitoring port stays on loopback, since only this machine's readiness checks read it. The store is a named
/// volume, so what is queued for an agent outlives a restart.
fn backplane_run_body(cluster_name: &str) -> String {
    let client = Services::Backplane.port();
    let monitor = Services::BackplaneMonitor.port();
    format!(
        concat!(
            "image={image}\n",
            "container={container}\n",
            "ports=\"-p {client}:{client} -p 127.0.0.1:{monitor}:{monitor}\"\n",
            "# $ports is a word-split flag list by design; do not quote it.\n",
            "docker run --rm --name \"$container\" $ports -v \"$container:{data}\" \"$image\" {args}\n",
        ),
        args = backplane_args().join(" "),
        client = client,
        container = container(BACKPLANE_TASK, cluster_name),
        data = DATA_DIR,
        image = BACKPLANE_IMAGE,
        monitor = monitor,
    )
}

/// The body of the task that starts the rustfs container, its S3 port open to the other machines' hubs.
///
/// `-e RUSTFS_SECRET_KEY` passes the name only, so Docker forwards the value the `[env] _.file` loaded, as the
/// collector task does with its password.
fn shared_storage_run_body(cluster_name: &str) -> String {
    let settings = shared_storage_env()
        .into_iter()
        .map(|(name, value)| format!("-e {name}={value}"))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        concat!(
            "image={image}\n",
            "container={container}\n",
            "settings=\"{settings} -e RUSTFS_SECRET_KEY\"\n",
            "start='{start}'\n",
            "# $settings is a word-split flag list by design; do not quote it.\n",
            "docker run --rm --name \"$container\" -p {port}:{port} -v \"$container:{data}\" $settings ",
            "--entrypoint sh \"$image\" -c \"$start\"\n",
        ),
        container = container(SHARED_STORAGE_TASK, cluster_name),
        data = DATA_DIR,
        image = SHARED_STORAGE_IMAGE,
        port = Services::SharedStorage.port(),
        settings = settings,
        start = shared_storage_start(),
    )
}

/// The body of the task that starts the hub.
///
/// A published deployment names its modules once, as the `[tools]` that stage them, and says nothing about where they
/// land: the hub asks mise, which is on `PATH` because the deployment is a mise config. Listing them again as paths
/// would be the same set written twice, in a form the generator cannot produce anyway. A local deployment has no such
/// list to lean on -- its modules are directories in a checkout -- so it still spells them out. The exception is a
/// scenario with modules of its own, which [`published_module_paths`] covers.
#[expect(
    clippy::single_call_fn,
    reason = "distinct step of generate_mise_deployment, split out to keep it readable"
)]
fn ws_server_run_body(
    scenario: &ScenarioModules<'_>,
    artifacts: ArtifactSource,
    output_abs: &Path,
) -> Result<String, CliError> {
    let (hub_command, module_paths) = if matches!(artifacts, ArtifactSource::Published) {
        (HUB_CRATE, published_module_paths(scenario, output_abs)?)
    } else {
        ("cargo run", scenario_module_paths(scenario)?)
    };
    if module_paths.is_empty() {
        return Ok(format!("{hub_command}\n"));
    }
    let module_paths_lines = wrap_module_paths(&module_paths);
    Ok(format!("{module_paths_lines}export MODULES_PATHS\n{hub_command}\n"))
}

/// The hub task's environment.
///
/// It names the page served at `/`: the hub has no default for it -- that would be one project's module name carried by
/// every other -- so the deployment that knows the answer states it.
///
/// A published deployment also names the agent store. The hub's own default is a directory of this repository's
/// layout (`services/ws-server/storage`) under whatever it takes for the project root -- outside this repository,
/// the directory it was started in. A published hub starts in the deployment directory, so the store is named there
/// directly, as the compose file does with its `/app/storage` volume. A local hub keeps the default, which is where
/// this repository's harnesses read agent output from. Hubs that `shared` storage are pointed at it by the caller
/// instead.
#[expect(
    clippy::single_call_fn,
    reason = "distinct step of generate_mise_deployment, split out to keep it readable"
)]
fn ws_server_env(scenario: &ScenarioModules<'_>, artifacts: ArtifactSource, shared: bool) -> Table {
    let mut env = hub_root_env(scenario.ws_server_dir, scenario.serves_a_page);
    if !shared && matches!(artifacts, ArtifactSource::Published) {
        let _previous: Option<Value> = env.insert(
            "STORAGE_URL".to_string(),
            Value::String(format!("file://{{{{ config_root }}}}/{STORAGE_DIR}")),
        );
    }
    env
}

/// The body of the task that starts the collector container.
///
/// The image, the container name and the settings are lifted into shell variables rather than folded with
/// continuations: inlining them would put the `docker run` past the editorconfig line length, and a wrapped copy is
/// what once silently dropped `-it`. `-e ZO_ROOT_USER_PASSWORD` passes the name only, so Docker forwards the value from
/// the task environment that `[env] _.file` loaded; the settings before it carry their values.
fn openobserve_run_body(cluster_name: &str) -> String {
    let collector_flags = COLLECTOR_SETTINGS
        .iter()
        .map(|(name, value)| format!("-e {name}={value}"))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        concat!(
            "image=openobserve/openobserve:v1.1.2\n",
            "container={container}\n",
            "settings=\"{flags}\"\n",
            "# $settings is a word-split flag list by design; do not quote it.\n",
            "docker run --rm --name \"$container\" -p 127.0.0.1:5080:5080 $settings ",
            "-e ZO_ROOT_USER_PASSWORD \"$image\"\n",
        ),
        container = collector_container(cluster_name),
        flags = collector_flags,
    )
}

/// The scenario's modules, the page first when there is one, each named as the hub serves it.
///
/// Nothing else is added: whatever a module needs at run time -- the wasm agent included, which it reaches through the
/// page it declares -- comes in as one of its declared dependencies.
fn scenario_modules(module_names: &[String], serves_a_page: bool) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    if serves_a_page {
        names.push(PAGE_MODULE.to_string());
    }
    names.extend(module_names.iter().cloned());
    names
}

/// The hub task's environment: the module to serve at `/`, for a cluster that serves one.
///
/// Empty otherwise, rather than naming the page anyway: the hub rejects a root it cannot find among the modules it was
/// given, and a headless cluster is not given the page.
fn hub_root_env(ws_server_dir: &Path, serves_a_page: bool) -> Table {
    let mut env = Table::new();
    if serves_a_page {
        let declared = super::hub_root_module(ws_server_dir);
        let _previous: Option<Value> = env.insert("MODULES_ROOT".to_string(), Value::String(declared));
    }
    env
}

/// The mise tools that stage every module this scenario serves.
///
/// This is the whole of what a published deployment says about its modules: the tools install them, and the hub serves
/// what the config staged. Nothing here records where a module lands, because that is decided per backend and platform
/// when the tool installs and so cannot be written down as the deployment is generated.
///
/// A module of this repository is staged as the package its `package.json` declares, which already carries the owner
/// scope the registry requires. Anything staged by a mise tool keeps the tool it was registered with, which is somebody
/// else's and must not be rewritten. A module from the scenario's own `module_paths:` is not staged at all: it has no
/// release, and is served from its directory instead.
pub(crate) fn staged_modules(scenario: &ScenarioModules<'_>) -> Result<Vec<String>, CliError> {
    let (registry, modules) = registry_and_modules(scenario);
    Ok(resolve_cluster_modules(&registry, &modules)?
        .into_iter()
        .filter_map(|entry| match entry.source {
            ModuleSource::Repo(_) => {
                let declared = entry.package_name.unwrap_or_default();
                Some(format!("{STAGED_PREFIX}{declared}"))
            }
            ModuleSource::MiseTool { tool, .. } => Some(tool),
            ModuleSource::Scenario { .. } => None,
        })
        .collect())
}

/// The `MODULES_PATHS` a published hub is started with, in resolution order; empty when it needs none.
///
/// Needed only once the scenario has a module from its own `module_paths:`, which has no release to stage and so is
/// handed over as a directory, relative to the deployment because a published hub runs from there. Setting the variable
/// replaces the hub's default paths, and of what those defaulted to, discovery restores only the `npm:` tools -- so
/// every other staged tool, `http:pyodide` among them, is named alongside the directories by the same `mise where` a
/// local deployment uses.
fn published_module_paths(scenario: &ScenarioModules<'_>, output_abs: &Path) -> Result<Vec<String>, CliError> {
    let (registry, modules) = registry_and_modules(scenario);
    let resolved = resolve_cluster_modules(&registry, &modules)?;
    if !resolved
        .iter()
        .any(|entry| matches!(entry.source, ModuleSource::Scenario { .. }))
    {
        return Ok(Vec::new());
    }
    let mut paths: Vec<String> = resolved
        .into_iter()
        .filter_map(|entry| match entry.source {
            ModuleSource::Scenario { dir, .. } => Some(relative_path_from(output_abs, &dir)),
            ModuleSource::MiseTool { tool, .. } if !tool.starts_with(STAGED_PREFIX) => Some(entry.mise_path),
            ModuleSource::Repo(_) | ModuleSource::MiseTool { .. } => None,
        })
        .collect();
    paths.sort();
    Ok(paths)
}

/// What a scenario's module resolution starts from, which is the same three answers either way it resolves.
///
/// One struct rather than three parameters because they are never apart: a module list means nothing without the tree
/// it was read from, and whether the page is among them is a property of the same scenario.
#[non_exhaustive]
pub struct ScenarioModules<'scenario> {
    /// The hub's directory, which is where the page module and the relative paths are resolved from.
    pub ws_server_dir: &'scenario Path,
    /// The modules the scenario itself declares, before their dependencies are resolved.
    pub module_names: &'scenario [String],
    /// Whether a browser opens this cluster, and so whether the page module is one of them.
    pub serves_a_page: bool,
    /// The scenario's own `module_paths:`, registered on top of this repository's modules.
    pub module_paths: &'scenario [PathBuf],
}

impl<'scenario> ScenarioModules<'scenario> {
    /// The three answers together, which is the only way a caller ever has them.
    #[must_use]
    pub const fn new(ws_server_dir: &'scenario Path, module_names: &'scenario [String], serves_a_page: bool) -> Self {
        Self {
            ws_server_dir,
            module_names,
            serves_a_page,
            module_paths: &[],
        }
    }

    /// The same scenario, also resolving against the module directories it names itself.
    #[must_use]
    pub const fn with_module_paths(self, module_paths: &'scenario [PathBuf]) -> Self {
        Self { module_paths, ..self }
    }
}

/// The registry to resolve against, and the module list to resolve -- what both resolutions here start from.
pub(super) fn registry_and_modules(
    scenario: &ScenarioModules<'_>,
) -> (BTreeMap<String, ModuleRegistryEntry>, Vec<String>) {
    let project_root = edge_toolkit::config::get_project_root();
    (
        module_registry(&project_root, scenario.ws_server_dir, scenario.module_paths),
        scenario_modules(scenario.module_names, scenario.serves_a_page),
    )
}

pub fn scenario_module_paths(scenario: &ScenarioModules<'_>) -> Result<Vec<String>, CliError> {
    let (registry, modules) = registry_and_modules(scenario);
    resolve_module_paths(&registry, &modules, |entry| entry.mise_path.clone())
}

// Pack `paths` into a run of `MODULES_PATHS=` assignments within the editorconfig line length, via textwrap first-
// fit bin-packing. Each path is one atomic fragment (some hold spaces, e.g. `$(mise where ...)`, so they must never
// be split) and paths sharing a line are joined by `, `. Each line after the first appends to the variable instead
// of continuing it with a trailing `\`, so the assembled value is the same comma-separated list the consumer's per-
// segment trim expects while the body stays free of line-continuations -- a `\` that picks up trailing whitespace
// silently ends the statement early, and the repo bans the form outside README files. The two fit budgets are
// the line length minus each form's fixed prefix and its closing quote: `MODULES_PATHS="` for the opening line,
// `MODULES_PATHS="$MODULES_PATHS, ` for every later one.
pub(super) fn wrap_module_paths(paths: &[String]) -> String {
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

pub(super) fn mise_task(
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

/// The `[env]` table that loads the scenario credential for every task in the file.
///
/// `_.file` is mise's env-file directive, resolved relative to this config, so both the OTLP variables the ws-server
/// needs and the root password the `OpenObserve` task passes through to Docker arrive from one place.
///
/// Built as a nested `_` table, which is what makes it a *dotted* key. Inserting the string `"_.file"` instead produces
/// the quoted key `"_.file" = "secrets.env"`, a single key whose name happens to contain a dot -- and mise reads that
/// as an ordinary variable, exporting `_.file=secrets.env` and loading nothing.
fn mise_env(artifacts: ArtifactSource) -> Table {
    let mut file = Table::new();
    let _previous: Option<Value> = file.insert("file".to_string(), Value::String(crate::SECRETS_ENV_FILE.to_string()));
    let mut env = Table::new();
    let _previous: Option<Value> = env.insert("_".to_string(), Value::Table(file));
    // The account beside the password, at the same scope, because whatever reads one has to read the other. The hub
    // does, and so does the WASI runner -- the only runner carrying an `OtlpConfig`; the web and pyo3 runners ignore
    // `OTLP_*` entirely. Handed a password with no account to present it as, the WASI runner fails at startup with
    // `missing field `username``. File scope rather than per task because the password already arrives there, from the
    // credential file, and splitting the pair is what broke it.
    let _previous: Option<Value> = env.insert(
        "OTLP_AUTH_USERNAME".to_string(),
        Value::String(COLLECTOR_USERNAME.to_string()),
    );
    if matches!(artifacts, ArtifactSource::Published) {
        // `{{ config_root }}` is the directory holding this file, so the reference survives the deployment being
        // generated anywhere and moved anywhere afterwards.
        let _previous: Option<Value> = env.insert(
            "NPM_CONFIG_USERCONFIG".to_string(),
            Value::String(format!("{{{{ config_root }}}}/{NPMRC_FILE}")),
        );
    }
    env
}

/// The npm config a published deployment carries, pointing the owner scope at GitHub Packages.
pub(crate) fn npmrc_contents() -> String {
    format!(
        "{scope}:registry=https://npm.pkg.github.com\n//npm.pkg.github.com/:_authToken=${{GITHUB_TOKEN}}\n",
        scope = MODULE_SCOPE.trim_end_matches('/'),
    )
}

/// Build a task's `depends` array, sorted.
///
/// The committed file is formatted with its arrays reordered, so emitting this one in the order the tasks happen to
/// be assembled leaves formatter and generator permanently at odds: each unsorts what the other sorted, and the drift
/// check reports whichever ran last. The order carries no meaning to mise either -- `depends` is a set of prerequisites
/// it starts together, not a sequence.
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

/// Render a runner task's body, which is just the runner.
///
/// No readiness wait here, deliberately. `depends` starts the hub and the runners together, so a runner can and does
/// win the race -- but the retry that handles it belongs in the runner's own module fetch, where it covers every
/// deployment shape. A wait bolted onto these tasks could never help the compose stack, whose runners gate on the hub's
/// `/health` and so can still arrive before the module scan finishes.
///
/// Two earlier attempts lived here and both were worse. A shell poll loop needed an HTTP client the generated
/// deployment could not guarantee (`xh` was declared in this file's own `[tools]`, but `task.run_auto_install` is off,
/// so nothing installed it and the task silently spun out its whole timeout). Replacing that with `et-cli wait-for-
/// module` fixed the tool problem but added a second `cargo run` to every runner task, which under the coverage profile
/// rebuilt the CLI before it could poll.
fn runner_run_body(runner: &RunnerInstance, artifacts: ArtifactSource) -> String {
    let crate_name = runner_crate(&runner.runner);
    let mut body = String::default();
    if matches!(artifacts, ArtifactSource::Published) {
        // The released binary is on `PATH` as a mise shim, so it needs no working directory of its own.
        let _write_result = writeln!(body, "{crate_name}");
        return body;
    }
    let _write_result = writeln!(body, "cargo run --quiet -p {crate_name}");
    body
}

/// The `[tools]` a generated deployment declares.
///
/// A local deployment declares nothing for what it runs, because `runner_run_body` and the hub task call only `cargo`,
/// which anyone building this repository already has. A published one names each released binary as a `cargo:` tool,
/// which is what puts it on `PATH`. `task.run_auto_install` is off, so these arrive through the `mise install` the
/// README asks for rather than on first use. What a staged module is requested at: `latest`, and for this project's own
/// modules a waived release age.
///
/// mise hides a release younger than `minimum_release_age` (24h by default) as supply-chain protection, so a deployment
/// generated alongside a fresh publish of its own modules could not install them for a day -- which is a day CI does
/// not have. The waiver is per tool rather than the global setting or a backend wildcard, both of which would also
/// exempt everyone else's packages: these are this project's own, published by the same release that produced this
/// deployment, so waiting on them protects against nothing. Third-party modules a scenario stages keep the full delay,
/// which is why this is decided per module here rather than once for the file.
fn module_tool_value(tool: &str, latest: &Value) -> Value {
    if !tool.starts_with(&format!("npm:{MODULE_SCOPE}")) {
        return latest.clone();
    }
    waived(latest)
}

/// The same waiver, for a tool named outright rather than resolved from a scenario's module list.
///
/// The hub and the runners are this project's own binaries, published by the release that produced the deployment, so
/// the reasoning above applies to them unchanged: a deployment generated beside a fresh publish would otherwise install
/// yesterday's binary for a day and give no sign of it, since resolving `latest` to an older release is what mise does
/// rather than an error. `cargo:open` is somebody else's and keeps the delay.
fn waived(latest: &Value) -> Value {
    let mut options = Table::new();
    let _previous: Option<Value> = options.insert("version".to_string(), latest.clone());
    let _previous: Option<Value> = options.insert("minimum_release_age".to_string(), Value::String("0".to_string()));
    Value::Table(options)
}

fn mise_tools(
    runners: &[RunnerInstance],
    artifacts: ArtifactSource,
    staged: &[String],
    with_process_compose: bool,
) -> Table {
    let mut tools = Table::new();
    let _previous: Option<Value> = tools.insert("cargo:open".to_string(), Value::String(LATEST.to_string()));
    // Somebody else's tool, so like `cargo:open` it keeps mise's release-age delay rather than taking a waiver.
    if with_process_compose {
        let _previous: Option<Value> =
            tools.insert(PROCESS_COMPOSE_TOOL.to_string(), Value::String(LATEST.to_string()));
    }
    if matches!(artifacts, ArtifactSource::Published) {
        let latest = Value::String(LATEST.to_string());
        let _previous: Option<Value> = tools.insert(format!("cargo:{HUB_CRATE}"), waived(&latest));
        for runner in runners {
            let crate_name = runner_crate(&runner.runner);
            let _previous: Option<Value> = tools.insert(format!("cargo:{crate_name}"), waived(&latest));
        }
        for tool in staged {
            let value = module_tool_value(tool, &latest);
            let _previous: Option<Value> = tools.insert(tool.clone(), value);
        }
    }
    tools
}

/// Working directory a task needs, or `None` when the command carries no dependence on where it runs.
///
/// A local deployment compiles out of the cargo workspace, so every task has to start there. A published one runs a
/// binary off `PATH` and only the hub still needs a directory, because the module paths it is handed are relative to
/// the server's own.
const fn workspace_dir(workspace_rel: &str, artifacts: ArtifactSource) -> Option<&str> {
    if matches!(artifacts, ArtifactSource::Published) {
        return None;
    }
    Some(workspace_rel)
}

/// The `RUNNER_*`/`WS_*` environment a runner task needs.
///
/// `WS_SERVER_URL` is spelled out rather than left to the runner's default so the generated task keeps working if that
/// default ever moves, and it is also what the runner derives its HTTP base from.
fn runner_env(runner: &RunnerInstance) -> Table {
    let mut env = Table::new();
    let _previous: Option<Value> = env.insert("RUNNER_MODULE".to_string(), Value::String(runner.module.clone()));
    let _previous: Option<Value> = env.insert("WS_SERVER_URL".to_string(), Value::String(runner.site.hub_ws_url()));
    // Whatever the scenario declared for this agent. It cannot collide with the two above: the resolver rejects an
    // `env:` naming either, so there is nothing here to decide between.
    for (name, value) in &runner.env {
        let _previous: Option<Value> = env.insert(name.clone(), Value::String(value.clone()));
    }
    env
}
