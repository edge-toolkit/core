#![expect(
    clippy::single_call_fn,
    reason = "et-cli decomposes scenario generation into named pipeline stages; each invoked once for readability"
)]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::slice;

use edge_toolkit::config::MODULE_TYPE_DIRS;
use edge_toolkit::ports::Services as Ports;
use et_path::{absolute_from, relative_path_from};
use et_sanitise::{is_plain_path, is_quotable_path};
use fs_err as fs;
use serde::{Deserialize, Serialize};

mod admin;
pub mod cli;
mod deployment_types;
mod error;
mod expose;
mod hub_ws_url;
mod input;
mod limits;
mod module_package_json;
mod placement;
mod readme;
mod recovery;
mod scenario_password;
mod schedule;
mod templates;

// `pub` here means "reachable from the binary or from `tests/`", and nothing else. This crate is a command line
// tool that happens to be split into a lib target so integration tests can drive it; the only consumer outside this
// directory is et-int-gen, which reads the `cli` tree to write HELP.md, and nothing exported is a promise. Everything
// the generators share among themselves is `pub(crate)`, so what remains below is the whole of the surface anyone could
// depend on -- short enough to read, which is what makes an accidental addition to it visible.
pub use self::admin::{EvictedAgent, block_ip, evict_agent};
pub use self::deployment_types::{ScenarioModules, docker_image_module_paths, scenario_module_paths};
pub(crate) use self::deployment_types::{
    generate_dekit_deployment, generate_docker_compose_deployment, generate_firmware_deployment,
    generate_k3s_deployment, generate_mise_deployment, generate_process_compose_deployment, generate_scenario_image,
};
pub use self::error::CliError;
pub use self::hub_ws_url::{hub_service_ws_url, hub_service_ws_url_named, hub_ws_url, hub_ws_url_at};
pub use self::input::{
    ArtifactSource, ClusterInput, DEFAULT_CLUSTER_NAME, Device, OutputType, Service, Services, infer_artifact_source,
    manifest_declares_this_repository,
};
pub(crate) use self::input::{Board, Delivery, running_in_this_repository};
pub use self::limits::{Cpus, Limits, LimitsError, Ulimit};
pub use self::module_package_json::generate_module_package_json;
pub use self::readme::scenario_dockerfile_path;
pub use self::recovery::{Recovery, RecoverySettings};
pub(crate) use self::scenario_password::{scenario_password, scenario_seed};
pub use self::schedule::Schedule;

/// Where the hub serves pyodide from, whichever distribution was selected.
const PYODIDE_DOCKER_PATH: &str = "/app/node_modules/pyodide";

/// Name of the generated env file that carries the scenario's derived credential.
///
/// The password is derived from the scenario input so a deployment is reproducible from it, which used to mean writing
/// the literal into `mise.toml` and `compose.yaml` -- both committed under `verification/`, where every secret scanner
/// duly found it. Collecting it into one file keeps the deployment reproducible while leaving the rest of the generated
/// output free of anything a scanner reads as a credential.
///
/// Whether that one file is committed depends on where it was generated, and [`write_deployment_gitignore`] decides:
/// under `verification/` it is a fixture the drift check reads, and anywhere else it is a real credential that the
/// ignore file written beside it lists.
pub(crate) const SECRETS_ENV_FILE: &str = "secrets.env";

/// Account the collector is created with, and the one the hub authenticates its OTLP exports as.
///
/// One constant because the two are the same account seen from either end: the collector is created with it as
/// `ZO_ROOT_USER_EMAIL` and the hub presents it as `OTLP_AUTH_USERNAME`, so a deployment where they disagree comes up
/// healthy and then rejects every export. Written in two files before this existed, with nothing checking that they
/// matched.
pub const COLLECTOR_USERNAME: &str = "root@example.com";

/// The collector's non-secret settings, which every generated deployment carries rather than sourcing.
///
/// Three formats render this -- a `ConfigMap`, a compose `environment:` block, a `docker run` flag list -- so it is
/// one list and a setting cannot reach some deployments and not others. Reading it from a file in this repository
/// instead is what made a generated deployment unable to leave the tree, for the sake of two values neither secret nor
/// scenario-specific.
///
/// Two things are deliberately absent. The root password is derived per scenario and reaches each format from the
/// generated env file. `ZO_DATA_DIR` is per format rather than shared, because it only means anything alongside the
/// storage that format declares -- a named volume, a claim, or nothing at all.
pub(crate) const COLLECTOR_SETTINGS: [(&str, &str); 2] =
    [("RUST_LOG", "warn"), ("ZO_ROOT_USER_EMAIL", COLLECTOR_USERNAME)];

/// Registry path the repository's own images are published under.
///
/// A runner image is the same for every deployment -- nothing in one varies by scenario -- so a scenario that asks for
/// published images names it here and the cluster pulls it, leaving the node with nothing to build or import. The hub
/// image is published alongside them but no manifest names it: it is the base a scenario image is layered onto, so it
/// reaches a deployment as a build context rather than as something a pod runs.
pub(crate) const IMAGE_REGISTRY: &str = et_org::IMAGE_REGISTRY;

/// Longest `cluster_name` that still leaves a valid Kubernetes namespace after the `et-` prefix.
///
/// A namespace is an RFC 1123 label, which the API server caps at 63 characters.
const CLUSTER_NAME_MAX: usize = 60;

/// Longest runner name, which names a Kubernetes `Deployment` as it is: an RFC 1123 label's 63 characters.
const RUNNER_NAME_MAX: usize = 63;

/// Directories of this repository whose every child holding a module is registered, relative to its root.
///
/// The module type directories are registered the same way, from `MODULE_TYPE_DIRS`.
const REPO_MODULE_PARENTS: [&str; 1] = ["data/model-modules"];

/// Single module directories of this repository, relative to its root, each registered as it is.
const REPO_MODULE_DIRS: [&str; 6] = [
    // Generated Python ws-modules: each generated/python-{ws,rest}/ holds its own pkg/package.json after `mise run
    // build-et-{ws,rest-client}- wheel`. They're listed individually because the parent `generated/` also contains non-
    // module artifacts (rust-rest, dart-ws, zig-rest, specs, docs).
    "generated/python-ws",
    "generated/python-rest",
    // The Python libraries, likewise listed one by one because `libs/` otherwise holds Rust crates.
    "libs/python-org",
    "libs/python-edge-toolkit",
    // The two the hub serves whatever the scenario asks for: its own page, and the agent that page loads. Registered
    // like any other module rather than prepended as bare paths by each generator, so the dependencies they declare are
    // resolved too. `static` names the runtimes its page pulls at boot, and a deployment that omits them serves a page
    // whose first import 404s.
    "services/ws-server/static",
    "services/ws-wasm-agent",
];

/// Registry scope this project's own module packages carry, and which the hub drops when naming a module.
pub(crate) const MODULE_SCOPE: &str = et_org::NPM_SCOPE;

/// Environment a runner gets from the deployment rather than from the scenario, and so cannot be overridden.
pub(crate) const DERIVED_RUNNER_ENV: [&str; 2] = ["RUNNER_MODULE", "WS_SERVER_URL"];

/// Runner kinds a scenario may name, mapped to the crate that runs them.
///
/// All three share one deployment shape, which is what lets one generator serve them: each takes the module's
/// published name in `RUNNER_MODULE`, fetches it from the hub named by `WS_SERVER_URL`, and builds from
/// `services/ws-<kind>-runner/Dockerfile`. Nothing else distinguishes a runner here, so a fourth is this line plus an
/// image. Rejecting a kind by name is what stops a scenario from asking for one and silently getting nothing.
pub(crate) const SUPPORTED_RUNNERS: [(&str, &str); 3] = [
    ("pyo3", "et-ws-pyo3-runner"),
    ("wasi", "et-ws-wasi-runner"),
    ("web", "et-ws-web-runner"),
];

/// Names the generated deployment already uses for its own tasks, services and aliases.
///
/// A runner is named after the agent that declares it, and both generators key on that name: mise inserts each task
/// into a table and compose writes each service as a mapping key. Either way a collision replaces rather than reports
/// -- an agent called `ws-server` would quietly take the hub's place, and the deployment would come up missing the
/// thing it was meant to talk to. Rejecting the name is the only way that surfaces.
pub(crate) const RESERVED_RUNNER_NAMES: [&str; 9] = [
    "generated-scenario",
    "o2",
    deployment_types::OPENER_TASK,
    deployment_types::COLLECTOR_TASK,
    deployment_types::HUB_TASK,
    deployment_types::HUB_IMAGE_BUILD,
    deployment_types::SCHEDULER_SERVICE,
    deployment_types::BACKPLANE_TASK,
    deployment_types::SHARED_STORAGE_TASK,
];

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct DeploymentSummary {
    pub cluster_name: String,
    pub agent_templates: usize,
    pub module_names: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct RegeneratedScenario {
    pub input_file: PathBuf,
    pub output_dir: PathBuf,
    pub summary: DeploymentSummary,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[non_exhaustive]
pub struct PackageJson {
    pub name: Option<String>,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
struct PyprojectPackage {
    project: Option<PyprojectProject>,
    tool: Option<PyprojectTool>,
}

#[derive(Debug, Default, Deserialize)]
struct PyprojectProject {
    name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct PyprojectTool {
    #[serde(rename = "ws-module")]
    ws_module: Option<PyprojectWsModule>,
}

#[derive(Debug, Default, Deserialize)]
struct PyprojectWsModule {
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
struct CargoPackage {
    package: Option<CargoPackageMetadata>,
}

#[derive(Debug, Default, Deserialize)]
struct CargoPackageMetadata {
    name: Option<String>,
    metadata: Option<CargoMetadata>,
}

#[derive(Debug, Default, Deserialize)]
struct CargoMetadata {
    #[serde(rename = "ws-module")]
    ws_module: Option<CargoWsModule>,
}

#[derive(Debug, Default, Deserialize)]
struct CargoWsModule {
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
}

/// Where a module's served files come from.
///
/// The deployment generators need this to tell apart the two provisioning routes: a repo directory can be copied
/// straight out of the Docker build context, whereas a mise-staged package exists only in the tool's install dir and
/// has to be installed before it can be staged.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub(crate) enum ModuleSource {
    /// A directory in this repository, as a path relative to the repository root.
    Repo(String),
    /// A package staged by a mise tool.
    ///
    /// Holds the backend-qualified tool id plus the published package name to locate beneath its install directory. The
    /// name rather than a path because the npm backend has no single layout: a package lands under `lib/node_modules/`,
    /// `node_modules/`, or an aube virtual store keyed by a content hash, depending on backend and platform, so the
    /// directory has to be found rather than assumed.
    MiseTool { tool: String, package: String },
    /// A module directory the scenario names in its own `module_paths:`, wherever that directory is.
    ///
    /// It belongs to whoever wrote the scenario, so there is no release of it to stage. A deployment that runs where
    /// the directory is reads it in place, from `dir`; an image copies it in from the named build context of the
    /// `module_paths:` entry it came from, `context`, at `context_path` within it.
    Scenario {
        dir: PathBuf,
        context: String,
        context_path: String,
    },
}

/// The named build context an image reads the `module_paths:` entry at `index` from.
///
/// Indexed rather than named after the directory, since two entries can end in the same name and a context name is the
/// only handle a `COPY --from=` has.
pub(crate) fn module_path_context(index: usize) -> String {
    format!("module-path-{index}")
}

/// Every `module_paths:` entry as the named build context that supplies it, with its path relative to `base`.
///
/// What a deployment building the scenario image hands to the build, alongside its main context.
pub(crate) fn module_path_contexts(module_paths: &[PathBuf], base: &Path) -> Vec<(String, String)> {
    module_paths
        .iter()
        .enumerate()
        .map(|(index, path)| (module_path_context(index), relative_path_from(base, path)))
        .collect()
}

/// Prefix that turns a bare image name into the one a scenario's `artifact_source` asks for.
///
/// A prefix rather than two parallel name builders, because that is the whole of the difference: the tags are
/// identical, and an unqualified one is what a container runtime treats as local-or-Docker-Hub.
#[must_use]
pub(crate) fn image_prefix(images: ArtifactSource) -> String {
    if matches!(images, ArtifactSource::Published) {
        format!("{IMAGE_REGISTRY}/")
    } else {
        String::default()
    }
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub(crate) struct ModuleRegistryEntry {
    pub mise_path: String,
    pub docker_path: String,
    pub dependencies: BTreeSet<String>,
    pub source: ModuleSource,
    /// The module's published package name, as `pkg/package.json` declares it.
    ///
    /// This is what a runner has to be told: `RUNNER_MODULE` is resolved against the names the hub serves modules
    /// under, which is the package name (`et-ws-math1`) and not the directory a scenario names it by (`math1`). `None`
    /// for a mise-staged package, which is already keyed by its published name.
    pub package_name: Option<String>,
}

pub fn generate_deployment(
    input_file: &Path,
    output_dir: &Path,
    output_type: Option<OutputType>,
) -> Result<DeploymentSummary, CliError> {
    let (cluster, seed) = load_cluster_input(input_file)?;
    // The flag wins where it is given; otherwise the input's own choice, which defaults to `Mise`.
    let output_type = output_type.unwrap_or(cluster.deployment_type);

    let module_names = cluster_module_names(&cluster);
    generate_deployment_outputs(&cluster, output_dir, &[output_type], seed)?;

    Ok(deployment_summary(
        cluster.cluster_name,
        cluster.agents.len(),
        module_names,
    ))
}

/// Load a scenario input together with the RNG seed derived from its bytes.
///
/// Read once and hashed here so the seed covers the exact file the deployment was generated from, comments and
/// formatting included, rather than a re-serialization of the parsed struct.
pub(crate) fn load_cluster_input(input_file: &Path) -> Result<(ClusterInput, u64), CliError> {
    let content = fs::read(input_file)?;
    let mut cluster: ClusterInput = serde_yaml::from_slice(&content)?;
    validate_cluster_name(&cluster.cluster_name)?;
    placement::validate_machines(&cluster)?;
    limits::validate_limits(deployment_types::HUB_TASK, &cluster.services.hub.limits)?;
    limits::validate_limits(deployment_types::COLLECTOR_TASK, &cluster.services.collector.limits)?;
    for (service, settings) in [
        ("backplane", &cluster.services.backplane),
        ("storage", &cluster.services.storage),
    ] {
        if !settings.is_unset() && !placement::shares_hubs(&cluster) {
            return Err(CliError::SharedServiceWithoutHubs {
                service: service.to_owned(),
            });
        }
    }
    limits::validate_limits(deployment_types::BACKPLANE_TASK, &cluster.services.backplane.limits)?;
    limits::validate_limits(deployment_types::SHARED_STORAGE_TASK, &cluster.services.storage.limits)?;
    for agent in &cluster.agents {
        // An agent's limits govern its runner processes; with no runner its modules run in a browser, which no
        // deployment format starts, so there would be nothing for them to apply to.
        if agent.runner.is_none() && !agent.limits.is_unset() {
            return Err(CliError::LimitsWithoutRunner {
                agent: agent.name.clone(),
            });
        }
        limits::validate_limits(&agent.name, &agent.limits)?;
        if !agent.schedule.is_unset() {
            if agent.runner.is_none() {
                return Err(CliError::ScheduleWithoutRunner {
                    agent: agent.name.clone(),
                });
            }
            schedule::validate_schedule(&agent.name, &agent.schedule)?;
        }
    }
    expose::validate(&cluster)?;
    // Relative to the input file, which is the one place a scenario can name its own modules from without knowing where
    // the CLI is run. The input itself is resolved the way `fs::read` just resolved it, against the working directory,
    // so the two cannot disagree when the CLI runs from a subdirectory of a repository. A path with no parent is a
    // filesystem root, which is its own directory.
    let input_abs = std::path::absolute(input_file)?;
    let input_dir = input_abs.parent().unwrap_or(&input_abs);
    let resolved: Vec<PathBuf> = cluster
        .module_paths
        .iter()
        .map(|path| absolute_from(input_dir, path))
        .collect();
    validate_module_paths(&cluster.module_paths, &resolved)?;
    cluster.module_paths = resolved;

    Ok((cluster, scenario_seed(&content)))
}

/// Reject a `cluster_name` that is not an RFC 1123 label.
///
/// The name reaches three renderers that each read it as trusted text: it is interpolated into generated comments,
/// into shell commands in the generated README, and into Kubernetes object names. The comment case is an instruction-
/// injection vector on its own -- a name carrying a newline closes the scenario Dockerfile's `# AUTO-GENERATED by ...`
/// comment and everything after it is read by `BuildKit` as further instructions. The README case is the same hazard
/// against a shell, where `;` or a backtick would end the `scenario=<name>` assignment and start a command. Rather
/// than escape per renderer, the name is held to the narrowest alphabet any of them needs, which is the one Kubernetes
/// already demands of a namespace: lowercase alphanumerics and `-`, starting and ending alphanumeric. That leaves
/// nothing to escape anywhere, and it turns a name the API server would have rejected at `kubectl apply` into an error
/// at generation time.
fn validate_cluster_name(name: &str) -> Result<(), CliError> {
    rfc1123_label(name, CLUSTER_NAME_MAX, |reason| CliError::InvalidClusterName {
        name: name.to_owned(),
        reason,
    })?;
    // The scenario's namespace is `et-<name>`, and one is already spoken for by the container runtime plugins.
    if format!("et-{name}") == deployment_types::K3S_PLUGIN_NAMESPACE {
        return Err(CliError::InvalidClusterName {
            name: name.to_owned(),
            reason: "its namespace is the one the container runtime plugins run in".to_owned(),
        });
    }
    Ok(())
}

/// Check `name` is an RFC 1123 label no longer than `max`, building the error from the reason it is not.
///
/// Lowercase alphanumerics and `-`, starting and ending alphanumeric: the alphabet Kubernetes demands of an object
/// name, and narrow enough that nothing a generated file interpolates it into needs to escape it. The caller supplies
/// `invalid` because the same rule guards different names, and each reports which one it was.
pub(crate) fn rfc1123_label<F>(name: &str, max: usize, invalid: F) -> Result<(), CliError>
where
    F: FnOnce(String) -> CliError,
{
    let reason = if name.is_empty() {
        "it is empty".to_owned()
    } else if name.len() > max {
        format!("it is longer than {max} characters")
    } else if let Some(bad) = name
        .chars()
        .find(|found| !(found.is_ascii_lowercase() || found.is_ascii_digit() || *found == '-'))
    {
        format!("it contains {bad:?}")
    } else if name.starts_with('-') || name.ends_with('-') {
        "it starts or ends with '-'".to_owned()
    } else {
        return Ok(());
    };
    Err(invalid(reason))
}

pub fn regenerate_verification(
    verification_root: &Path,
    output_type: Option<OutputType>,
) -> Result<Vec<RegeneratedScenario>, CliError> {
    let scenarios = discover_verification_scenarios(verification_root)?;

    let mut regenerated = Vec::with_capacity(scenarios.len());
    let mut seen_output_dirs = BTreeSet::new();
    for (input_file, output_dir) in scenarios {
        if !seen_output_dirs.insert(output_dir.clone()) {
            return Err(CliError::DuplicateScenarioOutput {
                root: verification_root.to_path_buf(),
                output: output_dir,
            });
        }

        let (cluster, seed) = load_cluster_input(&input_file)?;
        let module_names = cluster_module_names(&cluster);
        // A scheduled scenario is rendered only in the formats that can schedule it, rather than failing the whole run
        // on the ones that cannot; asked for one of those alone, it is left out.
        let scheduled = cluster.agents.iter().any(|agent| !agent.schedule.is_unset());
        let output_types: Vec<OutputType> = output_type
            .as_ref()
            .map_or(OutputType::ALL, std::slice::from_ref)
            .iter()
            .copied()
            .filter(|output_type| !scheduled || output_type.runs_schedules())
            .filter(|output_type| cluster.recovery.is_unset() || *output_type == OutputType::K3s)
            .filter(|output_type| {
                Board::ALL.iter().all(|board| {
                    board.spec().output_type != *output_type
                        || deployment_types::validate_firmware(&cluster, *board).is_ok()
                })
            })
            .filter(|output_type| deployment_types::validate_devices(&cluster, *output_type).is_ok())
            .collect();
        if output_types.is_empty() {
            continue;
        }

        generate_deployment_outputs(&cluster, &output_dir, &output_types, seed)?;
        let summary = deployment_summary(cluster.cluster_name, cluster.agents.len(), module_names);
        regenerated.push(RegeneratedScenario {
            input_file,
            output_dir,
            summary,
        });
    }

    Ok(regenerated)
}

const fn deployment_summary(
    cluster_name: String,
    agent_templates: usize,
    module_names: Vec<String>,
) -> DeploymentSummary {
    DeploymentSummary {
        cluster_name,
        agent_templates,
        module_names,
    }
}

/// Validate the requested formats and recovery bindings before writing or removing any deployment files.
fn validate_deployment_outputs(cluster: &ClusterInput, output_types: &[OutputType]) -> Result<(), CliError> {
    refuse_unschedulable_formats(cluster, output_types)?;
    recovery::validate_formats(cluster, output_types)?;
    recovery::validate(cluster, &scenario_runners(cluster)?)?;
    // After the runners resolve, so a scenario naming a runner that does not exist is told that rather than only that
    // the board runs none.
    for board in Board::ALL {
        if output_types.contains(&board.spec().output_type) {
            deployment_types::validate_firmware(cluster, *board)?;
        }
    }
    for output_type in output_types {
        deployment_types::validate_devices(cluster, *output_type)?;
    }
    Ok(())
}

/// Write every file a deployment of `output_types` needs into `output_dir`, with its credential and README.
fn generate_deployment_outputs(
    cluster: &ClusterInput,
    output_dir: &Path,
    output_types: &[OutputType],
    seed: u64,
) -> Result<(), CliError> {
    validate_deployment_outputs(cluster, output_types)?;
    if !output_dir.exists() {
        fs::create_dir_all(output_dir)?;
    }
    let with_process_compose = output_types.contains(&OutputType::ProcessCompose);

    // One password per scenario, shared by both deployment formats. OpenObserve and the ws-server have to agree on it:
    // the server authenticates its OTLP exports against the same root credentials the collector was started with.
    let password = scenario_password(seed);
    fs::write(
        output_dir.join(SECRETS_ENV_FILE),
        secrets_env(&password, placement::shares_hubs(cluster))?,
    )?;
    write_deployment_gitignore(output_dir)?;
    for output_type in output_types {
        generate_format(cluster, output_dir, *output_type, with_process_compose)?;
    }
    readme::write_readme(cluster, output_dir, output_types)?;
    recovery::remove_unsupported_outputs(cluster, output_dir)
}

/// Refuse a scheduled scenario in a format that cannot schedule it.
///
/// Checked before anything is written, so a scenario no requested format can schedule leaves no partial deployment.
fn refuse_unschedulable_formats(cluster: &ClusterInput, output_types: &[OutputType]) -> Result<(), CliError> {
    if let Some(output_type) = output_types.iter().find(|output_type| !output_type.runs_schedules())
        && let Some(agent) = cluster.agents.iter().find(|agent| !agent.schedule.is_unset())
    {
        return Err(CliError::ScheduleUnsupported {
            agent: agent.name.clone(),
            output_type: output_type.name().to_owned(),
        });
    }
    Ok(())
}

/// Write the files one deployment format consists of into `output_dir`.
///
/// `with_process_compose` is whether a `process-compose.yaml` is among the formats requested, which every `mise.toml`
/// written alongside has to declare the tool for, whichever format wrote it.
fn generate_format(
    cluster: &ClusterInput,
    output_dir: &Path,
    output_type: OutputType,
    with_process_compose: bool,
) -> Result<(), CliError> {
    match output_type {
        OutputType::Mise => generate_mise_deployment(cluster, output_dir, with_process_compose),
        OutputType::DockerCompose => {
            generate_docker_compose_deployment(cluster, output_dir)?;
            generate_scenario_image(cluster, output_dir)
        }
        // The scenario image is emitted here too, not just for compose. Both formats reference it: compose builds it
        // as a service, and the manifests name it as the hub's image with the README giving the `docker build` for it.
        // Generating k3s alone without it produced a README pointing at a Dockerfile that was never written.
        OutputType::K3s => {
            generate_k3s_deployment(cluster, output_dir)?;
            generate_scenario_image(cluster, output_dir)
        }
        // Every task it starts is a task of `mise.toml`, so that is written here too; generating `dekit.yaml` alone
        // would hand `dekit up` tasks that each run a file that does not exist.
        OutputType::Dekit => {
            generate_mise_deployment(cluster, output_dir, with_process_compose)?;
            generate_dekit_deployment(cluster, output_dir)
        }
        // The same, for the same reason: every process runs a `mise.toml` task. Written as a companion rather than a
        // deployment of its own, which is why a scheduled scenario still gets it here though `mise` alone refuses.
        OutputType::ProcessCompose => {
            generate_mise_deployment(cluster, output_dir, with_process_compose)?;
            generate_process_compose_deployment(cluster, output_dir)
        }
        OutputType::FirmwarePico2 => generate_firmware_deployment(cluster, output_dir, Board::Pico2),
        OutputType::FirmwareEsp32c6 => generate_firmware_deployment(cluster, output_dir, Board::Esp32c6),
        OutputType::FirmwarePicoPlus2 => generate_firmware_deployment(cluster, output_dir, Board::PicoPlus2),
    }
}

/// Keep a generated deployment's credential and runtime state out of whatever repository it was generated into.
///
/// Written beside the files it covers rather than left to the operator, because the failure is silent and permanent:
/// a credential committed once stays in the history after it is deleted. A nested `.gitignore` is what makes the
/// deployment directory safe to drop anywhere, which is the point of generating it.
///
/// Inside this repository the credential is left out. Here the verification outputs are committed evidence -- their
/// whole purpose is to be diffed when a generator changes -- and the password is derived from an input this repository
/// also carries, so it is reproducible from what is already public rather than a secret the file is keeping. Ignoring
/// it here would only hide it from the drift check that exists to read it. What the hub writes at runtime is ignored
/// everywhere, since it is never evidence of anything the generator did.
fn write_deployment_gitignore(output_dir: &Path) -> Result<(), CliError> {
    /// What the ignore file is rendered from.
    #[derive(Serialize)]
    struct Gitignore {
        /// Whether the credential is ignored too, which it is everywhere but in this repository.
        ignores_credential: bool,
        secrets_file: &'static str,
        storage_dir: &'static str,
    }
    let body = templates::render(
        "gitignore.j2",
        &Gitignore {
            ignores_credential: !running_in_this_repository(),
            secrets_file: SECRETS_ENV_FILE,
            storage_dir: deployment_types::STORAGE_DIR,
        },
    )?;
    fs::write(output_dir.join(".gitignore"), body)?;
    Ok(())
}

/// Render the env file every deployment format reads the scenario credential from.
///
/// Two names for the one password because the services that share it read different variables: `OpenObserve`
/// takes `ZO_ROOT_USER_PASSWORD` as its root credential, and the ws-server authenticates its OTLP exports with
/// `OTLP_AUTH_PASSWORD`. A cluster of several hubs adds the secret of the object store they share, under the name the
/// hubs' S3 client reads and the one rustfs reads. Nothing else belongs here. The accounts these authenticate as are
/// not secrets and each format states it outright, so keeping it in this file would have meant an uncommitted file
/// standing between a reader and a value there was never any reason to withhold -- and, in the Kubernetes case, a
/// `Secret` holding something that is not one.
fn secrets_env(password: &str, shares_hubs: bool) -> Result<String, CliError> {
    /// What the env file is rendered from: each variable the password is set as.
    #[derive(Serialize)]
    struct SecretsEnv<'password> {
        names: Vec<&'static str>,
        password: &'password str,
    }
    let mut names = vec!["ZO_ROOT_USER_PASSWORD", "OTLP_AUTH_PASSWORD"];
    if shares_hubs {
        names.extend(deployment_types::STORAGE_SECRET_ENVS);
    }
    templates::render("secrets.env.j2", &SecretsEnv { names, password })
}

fn discover_verification_scenarios(verification_root: &Path) -> Result<Vec<(PathBuf, PathBuf)>, CliError> {
    let mut scenarios = Vec::new();
    let verification_sets = fs::read_dir(verification_root)?;

    for entry in verification_sets {
        let entry = entry?;
        let set_root = entry.path();
        if !entry.file_type()?.is_dir() {
            continue;
        }

        let input_dir = set_root.join("input");
        let output_root = set_root.join("output");
        if !input_dir.is_dir() {
            continue;
        }

        let entries = fs::read_dir(&input_dir)?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            #[expect(
                clippy::filetype_is_file,
                reason = "scenario inputs are regular files only; dirs and symlinks are intentionally skipped"
            )]
            let is_regular_file = entry.file_type()?.is_file();
            if !is_regular_file {
                continue;
            }

            let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
                continue;
            };
            if extension != "yaml" {
                continue;
            }

            let Some(stem) = path.file_stem().map(PathBuf::from) else {
                return Err(CliError::MissingFileStem(path));
            };
            scenarios.push((path, output_root.join(stem)));
        }
    }

    if scenarios.is_empty() {
        return Err(CliError::NoScenarios(verification_root.to_path_buf()));
    }

    scenarios.sort_by(|(left, _), (right, _)| left.cmp(right));
    Ok(scenarios)
}

#[must_use]
pub(crate) fn module_registry(
    project_root: &Path,
    ws_server_dir: &Path,
    module_paths: &[PathBuf],
) -> BTreeMap<String, ModuleRegistryEntry> {
    let mut registry = BTreeMap::new();

    for parent in MODULE_TYPE_DIRS.into_iter().chain(REPO_MODULE_PARENTS) {
        register_modules_under(
            &mut registry,
            &project_root.join(parent),
            ws_server_dir,
            &format!("/app/{parent}"),
        );
    }
    for module in REPO_MODULE_DIRS {
        register_module_at(
            &mut registry,
            &project_root.join(module),
            ws_server_dir,
            &format!("/app/{module}"),
        );
    }

    register_external_module(
        &mut registry,
        "onnxruntime-web",
        "npm:onnxruntime-web",
        "/app/node_modules/onnxruntime-web",
    );
    // The GPU utilisation overlay on the hub's page, declared by `static` alongside onnxruntime-web.
    register_external_module(&mut registry, "stats-gl", "npm:stats-gl", "/app/node_modules/stats-gl");
    // Registered as the full distribution, which `resolve_cluster_modules` narrows to the much smaller npm package
    // for a cluster whose modules never call `micropip.install`. The full one comes from a GitHub release tarball that
    // mise's http backend extracts flat, so its install dir is itself the module directory.
    register_external_module(&mut registry, "pyodide", "http:pyodide", PYODIDE_DOCKER_PATH);

    // Last, so a scenario's own module cannot be shadowed by one of this repository's of the same name -- the scenario
    // named its directory deliberately, and silently serving something else would be the worse surprise.
    for (index, path) in module_paths.iter().enumerate() {
        register_scenario_paths(&mut registry, index, path, ws_server_dir);
    }

    registry
}

/// Whether a path holds a module itself, rather than being a parent of module directories.
fn is_module_dir(path: &Path) -> bool {
    path.join("pkg/package.json").is_file()
        || path.join("package.json").is_file()
        || path.join("Cargo.toml").is_file()
        || path.join("pyproject.toml").is_file()
}

/// Refuse a `module_paths:` directory whose name a generated deployment could not use as written.
///
/// Every generator puts these paths into shell commands, so a directory called `$(id)` would otherwise run `id` when
/// the deployment starts. Rather than escape the path for each generator, a name that is not plain is refused while
/// the scenario is read. What the scenario's author chose has to be plain: each entry as `written`, and each module
/// directory found under an entry, by its name. The directory the input sits in is wherever the scenario happens to
/// be, such as a Windows temporary directory under `RUNNER~1`, so the whole `resolved` path is held only to what the
/// generated commands need of it: that it reads back unchanged inside the double quotes and lists they put it in.
fn validate_module_paths(written: &[PathBuf], resolved: &[PathBuf]) -> Result<(), CliError> {
    for (index, (entry, path)) in written.iter().zip(resolved).enumerate() {
        if !is_plain_path(entry) {
            return Err(CliError::UnsafeModulePath(entry.clone()));
        }
        if !is_quotable_path(path) {
            return Err(CliError::UnsafeModulePath(path.clone()));
        }
        for (dir, ..) in scenario_module_dirs(index, path) {
            if dir != *path && !is_plain_path(Path::new(dir.file_name().unwrap_or_default())) {
                return Err(CliError::UnsafeModulePath(dir));
            }
        }
    }
    Ok(())
}

/// The module directories the `module_paths:` entry `path`, at `index`, names.
///
/// Each comes with its build-context path and the path an image holds it at. The entry is a module directory itself, or
/// a parent of several; a path that is neither names none.
fn scenario_module_dirs(index: usize, path: &Path) -> Vec<(PathBuf, String, String)> {
    let context_root = format!("/app/module-paths/{index}");
    if is_module_dir(path) {
        // The context is the module itself, so the whole of it is copied: its root, spelled as the one-character path.
        return vec![(path.to_path_buf(), '.'.to_string(), context_root)];
    }
    let Ok(entries) = fs::read_dir(path) else {
        return Vec::new();
    };
    let mut dirs: Vec<(PathBuf, String, String)> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|child| child.is_dir() && is_module_dir(child))
        .filter_map(|child| {
            let name = child.file_name()?.to_str()?.to_string();
            let docker_path = format!("{context_root}/{name}");
            Some((child, name, docker_path))
        })
        .collect();
    dirs.sort();
    dirs
}

/// Register the `module_paths:` entry at `index`, which is a module directory or a parent of several.
///
/// Each module lands at `/app/module-paths/<index>` in an image, below that at its own name when the entry is a parent.
/// The index keeps two entries ending in the same name apart, which matters beyond the image: the docker path is what
/// resolution deduplicates on.
fn register_scenario_paths(
    registry: &mut BTreeMap<String, ModuleRegistryEntry>,
    index: usize,
    path: &Path,
    ws_server_dir: &Path,
) {
    for (module_path, context_path, docker_path) in scenario_module_dirs(index, path) {
        let Some(directory_name) = module_path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let mut entry = module_entry(&module_path, ws_server_dir, &docker_path);
        entry.source = ModuleSource::Scenario {
            dir: module_path.clone(),
            context: module_path_context(index),
            context_path,
        };
        insert_module(registry, directory_name, entry);
    }
}

fn register_modules_under(
    registry: &mut BTreeMap<String, ModuleRegistryEntry>,
    root: &Path,
    ws_server_dir: &Path,
    docker_root: &str,
) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };

    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }

        let module_path = entry.path();
        let Some(directory_name) = module_path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        register_module(
            registry,
            &module_path,
            directory_name,
            ws_server_dir,
            &format!("{docker_root}/{directory_name}"),
        );
    }
}

/// Register a single module by its filesystem path (not a parent dir).
///
/// Used for modules that don't live under a module type directory -- currently the generated python clients under
/// `generated/`.
fn register_module_at(
    registry: &mut BTreeMap<String, ModuleRegistryEntry>,
    module_path: &Path,
    ws_server_dir: &Path,
    docker_path: &str,
) {
    // Absent outside this repository, where registering it anyway would record a module with no package name -- which a
    // published deployment then stages as the bare tool `npm:`.
    if !module_path.is_dir() {
        return;
    }
    let Some(directory_name) = module_path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    register_module(registry, module_path, directory_name, ws_server_dir, docker_path);
}

fn register_module(
    registry: &mut BTreeMap<String, ModuleRegistryEntry>,
    module_path: &Path,
    directory_name: &str,
    ws_server_dir: &Path,
    docker_path: &str,
) {
    insert_module(
        registry,
        directory_name,
        module_entry(module_path, ws_server_dir, docker_path),
    );
}

/// Add a module under its directory name and, when it declares one, the package name it is served as.
fn insert_module(
    registry: &mut BTreeMap<String, ModuleRegistryEntry>,
    directory_name: &str,
    entry: ModuleRegistryEntry,
) {
    if let Some(served_name) = entry.package_name.clone() {
        let _previous: Option<ModuleRegistryEntry> = registry.insert(served_name, entry.clone());
    }
    let _previous: Option<ModuleRegistryEntry> = registry.insert(directory_name.to_string(), entry);
}

/// The registry entry for a module directory of this repository.
fn module_entry(module_path: &Path, ws_server_dir: &Path, docker_path: &str) -> ModuleRegistryEntry {
    let package = module_package_json(module_path);
    // The docker path is always the repo-relative path under `/app`, which is where the hub image roots its module
    // scan, so stripping that prefix recovers the path to copy out of the build context.
    let repo_path = docker_path.strip_prefix("/app/").unwrap_or(docker_path).to_string();
    // The name a module is served, resolved and referred to by is the one its published `package.json` declares, scope
    // and all -- and it has to be that name whether or not `pkg/` has been built, because the lanes that generate a
    // deployment build no modules. A generated manifest already carries the scope; the source manifest standing in for
    // it when `pkg/` is absent (`Cargo.toml`, `pyproject.toml`) names the crate unscoped. So the rule that scopes a
    // dependency scopes the module's own name too, which leaves an already-scoped one untouched and keeps both sides of
    // a dependency edge spelling the same key.
    let served_name = package
        .as_ref()
        .and_then(|package| package.name.clone())
        .map(|name| module_package_json::scoped_npm_dependency_name(&name));
    ModuleRegistryEntry {
        mise_path: relative_path_from(ws_server_dir, module_path),
        docker_path: docker_path.to_string(),
        // Through the same scoping the generator applies when it writes a `package.json`, because the two have to name
        // the same module. A source manifest declares a dependency the way it declares its own crate -- unscoped -- and
        // publishing scopes both; reading one side raw would leave a dependency naming something the registry has no
        // key for.
        dependencies: package
            .as_ref()
            .map(|package| {
                package
                    .dependencies
                    .keys()
                    .map(|name| module_package_json::scoped_npm_dependency_name(name))
                    .collect()
            })
            .unwrap_or_default(),
        source: ModuleSource::Repo(repo_path),
        package_name: served_name,
    }
}

/// Register a package that mise stages outside the repository, keyed by its published package name.
///
/// The mise path is a shell substitution rather than a literal, because where a tool's install dir keeps the package
/// is not knowable when the deployment is generated. An archive-backed `http:` tool extracts flat, so `mise where`
/// is already the answer; the npm backend spreads packages across several layouts that differ by platform, so that
/// case defers to `et-cli npm-module-path`, which resolves it through the same code the ws-server uses to find these
/// packages itself.
fn register_external_module(
    registry: &mut BTreeMap<String, ModuleRegistryEntry>,
    package_name: &str,
    tool: &str,
    docker_path: &str,
) {
    let entry = external_module_entry(package_name, tool, docker_path);
    let _previous: Option<ModuleRegistryEntry> = registry.insert(package_name.to_string(), entry);
}

/// Build the registry entry for a mise-staged package.
///
/// Separate from registration so the pyodide swap can rebuild an entry for a different tool without restating how a
/// mise path is spelled.
fn external_module_entry(package_name: &str, tool: &str, docker_path: &str) -> ModuleRegistryEntry {
    // Resolved at run time, because where mise's npm backend puts a package varies by backend and platform. A local
    // deployment names the packages it wants rather than asking the hub to serve everything this config staged: the
    // repository's own tools table mixes modules with development tooling, so serving all of it would serve things that
    // are not modules at all. An archive-backed tool extracts flat, making its install directory the module directory,
    // which `mise where` answers outright.
    let mise_path = if tool.starts_with("npm:") {
        format!("$(cargo run --quiet -p et-cli -- npm-module-path --package {package_name})")
    } else {
        format!("$(mise where {tool})")
    };

    ModuleRegistryEntry {
        mise_path,
        docker_path: docker_path.to_string(),
        dependencies: BTreeSet::new(),
        source: ModuleSource::MiseTool {
            tool: tool.to_string(),
            package: package_name.to_string(),
        },
        package_name: None,
    }
}

#[must_use]
pub fn module_package_json(module_path: &Path) -> Option<PackageJson> {
    let pkg_package = read_package_json(&module_path.join("pkg/package.json"));
    let root_package = read_package_json(&module_path.join("package.json"));
    let pyproject = read_pyproject_package(&module_path.join("pyproject.toml"));
    let cargo_package = read_cargo_package(&module_path.join("Cargo.toml"));
    if pkg_package.is_none() && root_package.is_none() && pyproject.is_none() && cargo_package.is_none() {
        return None;
    }

    let mut package = pkg_package.or_else(|| root_package.clone()).unwrap_or_default();
    if let Some(root_package) = root_package {
        package.dependencies.extend(root_package.dependencies);
        if package.name.is_none() {
            package.name = root_package.name;
        }
    }
    if let Some(pyproject) = pyproject {
        if let Some(ws_module) = pyproject.tool.and_then(|tool| tool.ws_module) {
            package.dependencies.extend(ws_module.dependencies);
        }
        if package.name.is_none() {
            package.name = pyproject.project.and_then(|project| project.name);
        }
    }
    if let Some(cargo_package) = cargo_package.and_then(|cargo_package| cargo_package.package) {
        if let Some(ws_module) = cargo_package.metadata.and_then(|metadata| metadata.ws_module) {
            package.dependencies.extend(ws_module.dependencies);
        }
        if package.name.is_none() {
            package.name = cargo_package.name;
        }
    }
    Some(package)
}

fn read_package_json(path: &Path) -> Option<PackageJson> {
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn read_pyproject_package(path: &Path) -> Option<PyprojectPackage> {
    let content = fs::read_to_string(path).ok()?;
    toml::from_str(&content).ok()
}

fn read_cargo_package(path: &Path) -> Option<CargoPackage> {
    let content = fs::read_to_string(path).ok()?;
    toml::from_str(&content).ok()
}

/// Walk `module_names` and everything they depend on, in breadth-first declaration order.
///
/// A module is registered under both its directory name and its `package.json` name, so the same entry is reachable by
/// two keys; de-duplicating on the docker path collapses those without disturbing the order the generated files depend
/// on.
fn resolve_module_entries<'registry>(
    registry: &'registry BTreeMap<String, ModuleRegistryEntry>,
    module_names: &[String],
) -> Result<Vec<&'registry ModuleRegistryEntry>, CliError> {
    let mut entries = Vec::new();
    let mut queued: VecDeque<String> = module_names.iter().cloned().collect();
    let mut seen_keys = BTreeSet::new();
    let mut seen_paths = BTreeSet::new();

    while let Some(module_name) = queued.pop_front() {
        if !seen_keys.insert(module_name.clone()) {
            continue;
        }

        let entry = registry
            .get(&module_name)
            .ok_or_else(|| CliError::UnknownDependency(module_name.clone()))?;
        if seen_paths.insert(entry.docker_path.clone()) {
            entries.push(entry);
        }
        queued.extend(entry.dependencies.iter().cloned());
    }

    Ok(entries)
}

/// Resolve `module_names` and their dependencies to the path each is served from, as `path_for` spells it, sorted.
pub(crate) fn resolve_module_paths<F>(
    registry: &BTreeMap<String, ModuleRegistryEntry>,
    module_names: &[String],
    path_for: F,
) -> Result<Vec<String>, CliError>
where
    F: Fn(&ModuleRegistryEntry) -> String,
{
    // Sorted rather than left in resolution order, which is breadth-first from whichever modules the scenario names, so
    // a dependency lands wherever it was first reached and moves whenever an unrelated module gains one. The hub serves
    // modules by name, so the order carries nothing, and sorted it stays put in a committed deployment.
    let mut paths: Vec<String> = resolve_cluster_modules(registry, module_names)?
        .iter()
        .map(path_for)
        .collect();
    paths.sort();
    Ok(paths)
}

/// Resolve a cluster's modules, sized to what those modules actually need.
///
/// Everything is taken from the registry as-is except pyodide, whose distribution depends on the cluster: the registry
/// cannot decide that, because pyodide arrives as a dependency of whichever Python modules the cluster happens to
/// declare.
pub(crate) fn resolve_cluster_modules(
    registry: &BTreeMap<String, ModuleRegistryEntry>,
    module_names: &[String],
) -> Result<Vec<ModuleRegistryEntry>, CliError> {
    let resolved = resolve_module_entries(registry, module_names)?;
    let project_root = edge_toolkit::config::get_project_root();
    let needs_full_pyodide = resolved
        .iter()
        .any(|entry| module_installs_wheels(&project_root, entry));

    Ok(resolved
        .into_iter()
        .map(|entry| {
            if entry.docker_path == PYODIDE_DOCKER_PATH && !needs_full_pyodide {
                external_module_entry("pyodide", "npm:pyodide", PYODIDE_DOCKER_PATH)
            } else {
                entry.clone()
            }
        })
        .collect())
}

/// Whether a module pulls a non-stdlib wheel at runtime.
///
/// Decided from the module's served `pkg/`, which is the code the browser actually runs, rather than from its Python
/// sources: the `micropip.install` calls live in each Python module's JS loader shim.
fn module_installs_wheels(project_root: &Path, entry: &ModuleRegistryEntry) -> bool {
    // A `module_paths:` module may be served from its own root rather than a `pkg/`, so its loader is looked for in
    // whichever of the two the hub would serve.
    let served_dir = match &entry.source {
        ModuleSource::Repo(repo_path) => project_root.join(repo_path).join("pkg"),
        ModuleSource::Scenario { dir, .. } if dir.join("pkg").is_dir() => dir.join("pkg"),
        ModuleSource::Scenario { dir, .. } => dir.clone(),
        ModuleSource::MiseTool { .. } => return false,
    };
    let Ok(files) = fs::read_dir(served_dir) else {
        return false;
    };

    files.flatten().any(|file| {
        let path = file.path();
        if path.extension().is_none_or(|extension| extension != "js") {
            return false;
        }
        fs::read_to_string(&path).is_ok_and(|source| source.contains("micropip") && source.contains(".install("))
    })
}

/// Resolve the directory holding a mise-staged npm package.
///
/// Defers to the resolver the ws-server itself uses, which is the only place that knows the layouts mise's npm backend
/// produces. Generated deployments call back into this rather than embedding a path, because the layout differs per
/// platform and backend and so cannot be decided when the deployment is generated.
pub fn npm_module_path(package: &str) -> Result<PathBuf, CliError> {
    edge_toolkit::config::mise_npm_package_path(package)
        .ok_or_else(|| CliError::UnresolvedNpmModule(package.to_string()))
}

/// One runner process the generated deployment starts, resolved from an agent that names a `runner:`.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub(crate) struct RunnerInstance {
    /// Deployment-unique name for the process: the task name in `mise.toml`, the service name in `compose.yaml`.
    pub name: String,
    /// Runner kind the agent asked for, already validated against [`SUPPORTED_RUNNERS`].
    pub runner: String,
    /// Value for the runner's `RUNNER_MODULE`, i.e. the module's published package name.
    pub module: String,
    /// Extra environment the scenario declared for this agent, rendered by every deployment format.
    pub env: BTreeMap<String, String>,
    /// The machine the runner's agent is placed on.
    pub site: placement::Site,
    /// Resource limits the scenario set for this runner's agent.
    pub limits: Limits,
    /// When the runner is started; unset runs it continuously.
    pub schedule: Schedule,
    /// The ports the runner's agent publishes outside the cluster.
    pub expose: Vec<input::ExposedPort>,
}

/// Resolve every agent that names a `runner:` into the processes the deployment has to start.
///
/// One process per resource rather than per agent, because a runner hosts exactly one module -- `RUNNER_MODULE` is a
/// single name. An agent with one resource (the usual shape) therefore keeps the agent's own name, and only a multi-
/// resource agent gets the resource suffixed, so the common case reads as the scenario wrote it.
pub(crate) fn resolve_cluster_runners(
    registry: &BTreeMap<String, ModuleRegistryEntry>,
    cluster: &ClusterInput,
) -> Result<Vec<RunnerInstance>, CliError> {
    let mut runners = Vec::new();
    for agent in &cluster.agents {
        let Some(runner) = agent.runner.as_deref().map(str::trim).filter(|kind| !kind.is_empty()) else {
            continue;
        };
        if !SUPPORTED_RUNNERS.iter().any(|(kind, _)| *kind == runner) {
            let supported = SUPPORTED_RUNNERS
                .iter()
                .map(|(kind, _)| *kind)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(CliError::UnsupportedRunner {
                agent: agent.name.clone(),
                runner: runner.to_string(),
                supported,
            });
        }
        // A scenario cannot set the two the deployment derives for it. `RUNNER_MODULE` and `WS_SERVER_URL` are what
        // wire a runner to its module and its hub, and both are computed from the rest of the input. Letting `env:` win
        // would mean a scenario whose generated files describe one deployment and whose runners join another; letting
        // the derived value win would mean an `env:` entry that is silently ignored. Neither is worth allowing, so it
        // is an error to write one.
        for variable in DERIVED_RUNNER_ENV {
            if agent.env.contains_key(variable) {
                return Err(CliError::ReservedRunnerEnv {
                    agent: agent.name.clone(),
                    variable: (*variable).to_string(),
                });
            }
        }
        let multi_resource = agent.resources.len() > 1;
        for resource in &agent.resources {
            let module_name = resource.resource_type.trim();
            if module_name.is_empty() {
                continue;
            }
            let entry = registry
                .get(module_name)
                .ok_or_else(|| CliError::UnknownDependency(module_name.to_string()))?;
            // The hub serves a module under its package name, so that is what the runner has to ask for.
            let module = entry.package_name.clone().unwrap_or_else(|| module_name.to_string());
            let name = runner_name(&agent.name, module_name, multi_resource, &runners)?;
            // A machine is a mise task of its own as well, has a compose scheduler service and may run a hub, so a
            // runner taking any of those names would replace it.
            if cluster.machines.iter().any(|machine| {
                name == deployment_types::machine_task(&machine.name)
                    || name == deployment_types::scheduler_service(slice::from_ref(&machine.name))
            }) || placement::hubs(cluster).iter().any(|hub| hub.name == name)
            {
                return Err(CliError::ReservedRunnerName {
                    agent: agent.name.clone(),
                    name,
                });
            }
            runners.push(RunnerInstance {
                name,
                runner: runner.to_string(),
                module,
                // Every resource of a multi-resource agent gets its own runner process, and the agent's environment
                // describes the agent, so each of them carries it.
                env: agent.env.clone(),
                site: placement::agent_site(cluster, &agent.name),
                limits: agent.limits.clone(),
                schedule: agent.schedule.clone(),
                expose: agent.expose.clone(),
            });
        }
    }
    Ok(runners)
}

/// Every runner `cluster` declares, resolved against the modules this tree and the scenario's own paths hold.
pub(crate) fn scenario_runners(cluster: &ClusterInput) -> Result<Vec<RunnerInstance>, CliError> {
    let workspace_root = edge_toolkit::config::get_project_root();
    let ws_server_dir = workspace_root.join("services/ws-server");
    resolve_cluster_runners(
        &module_registry(&workspace_root, &ws_server_dir, &cluster.module_paths),
        cluster,
    )
}

/// The runners a format starts as tasks of its own: every runner but those on a board, which its firmware runs.
pub(crate) fn task_runners(cluster: &ClusterInput) -> Result<Vec<RunnerInstance>, CliError> {
    Ok(scenario_runners(cluster)?
        .into_iter()
        .filter(|runner| placement::delivery_at(cluster, &runner.site) == Delivery::Tasks)
        .collect())
}

/// Base HTTP URL a generated deployment reaches the hub on.
///
/// Every generator addresses the hub by its standard insecure port, so spelling the URL out in each of them meant
/// writing the same format string more than once. One definition here serves the compose services, the mise tasks and
/// whatever is added next, and it is the only place that has to change if the port moves.
#[must_use]
pub(crate) fn hub_http_base() -> String {
    format!("http://localhost:{}", Ports::InsecureWebSocketServer.port())
}

/// Derive one runner's deployment-unique name, rejecting the two ways it can collide.
///
/// An agent with a single resource keeps its own name, so the common case reads as the scenario wrote it; only a multi-
/// resource agent gets the resource suffixed, because each resource becomes its own process.
///
/// Both checks exist because a collision would otherwise be silent rather than wrong-looking: mise inserts each
/// task into a table and compose writes each service as a mapping key, so a repeated name replaces what was there. A
/// scenario could lose its hub and only find out when the runners had nothing to talk to.
#[expect(
    clippy::single_call_fn,
    reason = "distinct step of resolve_cluster_runners; separate to keep that function within its complexity budget"
)]
fn runner_name(
    agent_name: &str,
    module_name: &str,
    multi_resource: bool,
    existing: &[RunnerInstance],
) -> Result<String, CliError> {
    let name = if multi_resource {
        format!("{agent_name}-{module_name}")
    } else {
        agent_name.to_string()
    };
    // The name becomes a mise task, a `mise run` argument, a compose service and Kubernetes object names, so it is held
    // to the alphabet all of them accept: a name such as `--help` would otherwise reach `mise run` as a flag.
    rfc1123_label(&name, RUNNER_NAME_MAX, |reason| CliError::InvalidRunnerName {
        agent: agent_name.to_string(),
        name: name.clone(),
        reason,
    })?;
    if RESERVED_RUNNER_NAMES.contains(&name.as_str()) {
        return Err(CliError::ReservedRunnerName {
            agent: agent_name.to_string(),
            name,
        });
    }
    if existing.iter().any(|runner| runner.name == name) {
        return Err(CliError::DuplicateRunnerName { name });
    }
    Ok(name)
}

/// The crate whose binary runs `runner`, which [`resolve_cluster_runners`] has already validated.
#[must_use]
pub(crate) fn runner_crate(runner: &str) -> &'static str {
    SUPPORTED_RUNNERS
        .iter()
        .find(|(kind, _)| *kind == runner)
        .map_or("et-ws-web-runner", |(_, crate_name)| *crate_name)
}

#[must_use]
pub(crate) fn cluster_module_names(cluster: &ClusterInput) -> Vec<String> {
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
