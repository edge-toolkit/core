use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use et_path::{absolute_from, relative_path_from};
use fs_err as fs;
use toml::{Table, Value};

use crate::error::CliError;
use crate::input::{ArtifactSource, ClusterInput};
use crate::{
    COLLECTOR_SETTINGS, COLLECTOR_USERNAME, MODULE_SCOPE, ModuleRegistryEntry, ModuleSource, RunnerInstance,
    cluster_module_names, hub_ws_url, module_registry, resolve_cluster_modules, resolve_cluster_runners,
    resolve_module_paths, runner_crate,
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

/// Version every `cargo:` tool the generated deployment declares is requested at.
const LATEST: &str = "latest";

pub fn generate_mise_deployment(cluster: &ClusterInput, output_dir: &Path) -> Result<(), CliError> {
    let output_path = output_dir.join("mise.toml");
    let workspace_root = edge_toolkit::config::get_project_root();
    let output_abs = absolute_from(&workspace_root, output_dir);
    let ws_server_dir = workspace_root.join("services/ws-server");
    let workspace_rel = relative_path_from(&output_abs, &workspace_root);
    let openobserve_run = openobserve_run_body();
    let module_names = cluster_module_names(cluster);
    let artifacts = cluster.artifact_source;
    let scenario = ScenarioModules::new(&ws_server_dir, &module_names, super::serves_a_page(cluster));
    let staged = if matches!(artifacts, ArtifactSource::Published) {
        staged_modules(&scenario)?
    } else {
        Vec::new()
    };
    let hub_command = if matches!(artifacts, ArtifactSource::Published) {
        HUB_CRATE
    } else {
        "cargo run"
    };
    // A published deployment names its modules once, as the `[tools]` that stage them, and says nothing about where
    // they land: the hub asks mise, which is on `PATH` because the deployment is a mise config. Listing them again as
    // paths would be the same set written twice, in a form the generator cannot produce anyway. A local deployment has
    // no such list to lean on -- its modules are directories in a checkout -- so it still spells them out.
    let ws_server_run = if matches!(artifacts, ArtifactSource::Published) {
        format!("{hub_command}\n")
    } else {
        let module_paths = scenario_module_paths(&scenario)?;
        let module_paths_lines = wrap_module_paths(&module_paths);
        format!("{module_paths_lines}export MODULES_PATHS\n{hub_command}\n")
    };
    let ws_server_rel = relative_path_from(&output_abs, &ws_server_dir);
    // A published hub is handed absolute paths that `mise where` resolves, so it needs no directory of its own; a local
    // one is given paths relative to the server's source dir and has to start there.
    let ws_server_dir_entry = workspace_dir(&ws_server_rel, artifacts);
    // Which module is the page served at `/`. The hub has no default for it -- that would be one project's module name
    // carried by every other -- so the deployment that knows the answer states it.
    let ws_server_env = Some(hub_root_env(&ws_server_dir, scenario.serves_a_page));

    let mut root = Table::new();
    let mut tasks = Table::new();

    // No working directory: the collector is a container started from values this file carries, so unlike the hub it
    // has nothing to resolve against the repository.
    let _previous: Option<Value> = tasks.insert(
        "openobserve".to_string(),
        Value::Table(mise_task(Some("o2"), None, None, Some(&openobserve_run), None, None)),
    );
    let _previous: Option<Value> = tasks.insert(
        "ws-server".to_string(),
        Value::Table(mise_task(
            None,
            Some("Run the WebSocket server"),
            ws_server_dir_entry,
            Some(&ws_server_run),
            None,
            ws_server_env,
        )),
    );
    // Each runner agent becomes its own task, and `generated-scenario` depends on all of them. mise runs `depends`
    // concurrently, which is what a cluster wants -- the hub and every runner are long-running peers, not a pipeline --
    // but it also means a runner starts before the hub is listening. Each runner body therefore waits for the hub's own
    // health endpoint first; see `runner_run_body`.
    let runners = resolve_cluster_runners(&module_registry(&workspace_root, &ws_server_dir), cluster)?;
    for runner in &runners {
        let _previous: Option<Value> = tasks.insert(
            runner.name.clone(),
            Value::Table(mise_task(
                None,
                Some(&format!("Run {} in the {} runner", runner.module, runner.runner)),
                workspace_dir(&workspace_rel, artifacts),
                Some(&runner_run_body(runner, artifacts)),
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

    let _previous: Option<Value> = root.insert("env".to_string(), Value::Table(mise_env(artifacts)));
    let _previous: Option<Value> = root.insert("tasks".to_string(), Value::Table(tasks));

    let tools = mise_tools(&runners, artifacts, &staged);
    let _previous: Option<Value> = root.insert("tools".to_string(), Value::Table(tools));

    fs::write(&output_path, toml::to_string(&Value::Table(root))?)?;
    if matches!(artifacts, ArtifactSource::Published) {
        fs::write(output_dir.join(NPMRC_FILE), npmrc_contents())?;
    }

    Ok(())
}

/// The body of the task that starts the collector container.
///
/// The image and the settings are lifted into shell variables rather than folded with continuations: inlining them
/// would put the `docker run` past the editorconfig line length, and a wrapped copy is what once silently dropped
/// `-it`. `-e ZO_ROOT_USER_PASSWORD` passes the name only, so Docker forwards the value from the task environment that
/// `[env] _.file` loaded; the settings before it carry their values.
fn openobserve_run_body() -> String {
    let collector_flags = COLLECTOR_SETTINGS
        .iter()
        .map(|(name, value)| format!("-e {name}={value}"))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        concat!(
            "image=openobserve/openobserve:v0.91.5\n",
            "settings=\"{}\"\n",
            "# $settings is a word-split flag list by design; do not quote it.\n",
            "docker run --rm --name openobserve -p 127.0.0.1:5080:5080 $settings ",
            "-e ZO_ROOT_USER_PASSWORD \"$image\"\n",
        ),
        collector_flags
    )
}

/// The agent every module talks to the hub through, served whatever the scenario declares.
///
/// Named rather than prepended as a path, so it is resolved through the registry like any other module and the
/// dependencies it declares come with it. It is not conditional the way the page is: a module run by a headless runner
/// loads it too, and the modules that need it do not all declare it.
const BASE_MODULES: [&str; 1] = ["ws-wasm-agent"];

/// The page served at `/`, which only a cluster a browser opens has any use for.
///
/// `static` names the runtimes its page imports at boot, so a deployment that serves the page without it serves one
/// whose first import 404s -- and one that serves neither is a headless cluster that was never going to load either.
const PAGE_MODULE: &str = "static";

/// The scenario's modules, the base ones first, each named as the hub serves it.
fn scenario_modules(module_names: &[String], serves_a_page: bool) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    if serves_a_page {
        names.push(PAGE_MODULE.to_string());
    }
    names.extend(BASE_MODULES.iter().map(|name| (*name).to_string()));
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
pub(crate) fn staged_modules(scenario: &ScenarioModules<'_>) -> Result<Vec<String>, CliError> {
    let (registry, modules) = registry_and_modules(scenario);
    Ok(resolve_cluster_modules(&registry, &modules)?
        .iter()
        .map(staged_module)
        .collect())
}

/// What a scenario's module resolution starts from, which is the same three answers either way it resolves.
///
/// One struct rather than three parameters because they are never apart: a module list means nothing without
/// the tree it was read from, and whether the page is among them is a property of the same scenario.
#[non_exhaustive]
pub struct ScenarioModules<'scenario> {
    /// The hub's directory, which is where the page module and the relative paths are resolved from.
    pub ws_server_dir: &'scenario Path,
    /// The modules the scenario itself declares, before the base ones are added.
    pub module_names: &'scenario [String],
    /// Whether a browser opens this cluster, and so whether the page module is one of them.
    pub serves_a_page: bool,
}

impl<'scenario> ScenarioModules<'scenario> {
    /// The three answers together, which is the only way a caller ever has them.
    #[must_use]
    pub const fn new(ws_server_dir: &'scenario Path, module_names: &'scenario [String], serves_a_page: bool) -> Self {
        Self {
            ws_server_dir,
            module_names,
            serves_a_page,
        }
    }
}

/// The registry to resolve against, and the module list to resolve -- what both resolutions here start from.
fn registry_and_modules(scenario: &ScenarioModules<'_>) -> (BTreeMap<String, ModuleRegistryEntry>, Vec<String>) {
    let project_root = edge_toolkit::config::get_project_root();
    (
        module_registry(&project_root, scenario.ws_server_dir),
        scenario_modules(scenario.module_names, scenario.serves_a_page),
    )
}

/// The tool that stages one resolved module, which differs by where the module comes from.
///
/// A module of this repository is staged as the package its `package.json` declares, which already carries the owner
/// scope the registry requires. Anything staged by a mise tool keeps the tool it was registered with, which is somebody
/// else's and must not be rewritten.
fn staged_module(entry: &ModuleRegistryEntry) -> String {
    match &entry.source {
        ModuleSource::Repo(_) => {
            let declared = entry.package_name.clone().unwrap_or_default();
            format!("{STAGED_PREFIX}{declared}")
        }
        ModuleSource::MiseTool { tool, .. } => tool.clone(),
    }
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
fn npmrc_contents() -> String {
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
/// The hub and the runners are this project's own binaries, published by the release that produced the
/// deployment, so the reasoning above applies to them unchanged: a deployment generated beside a fresh publish
/// would otherwise install yesterday's binary for a day and give no sign of it, since resolving `latest` to an
/// older release is what mise does rather than an error. `cargo:open` is somebody else's and keeps the delay.
fn waived(latest: &Value) -> Value {
    let mut options = Table::new();
    let _previous: Option<Value> = options.insert("version".to_string(), latest.clone());
    let _previous: Option<Value> = options.insert("minimum_release_age".to_string(), Value::String("0".to_string()));
    Value::Table(options)
}

fn mise_tools(runners: &[RunnerInstance], artifacts: ArtifactSource, staged: &[String]) -> Table {
    let mut tools = Table::new();
    let _previous: Option<Value> = tools.insert("cargo:open".to_string(), Value::String(LATEST.to_string()));
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
    let _previous: Option<Value> = env.insert("WS_SERVER_URL".to_string(), Value::String(hub_ws_url()));
    // Whatever the scenario declared for this agent. It cannot collide with the two above: the resolver rejects an
    // `env:` naming either, so there is nothing here to decide between.
    for (name, value) in &runner.env {
        let _previous: Option<Value> = env.insert(name.clone(), Value::String(value.clone()));
    }
    env
}
