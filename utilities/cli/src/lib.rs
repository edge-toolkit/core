#![expect(
    clippy::single_call_fn,
    reason = "et-cli decomposes scenario generation into named pipeline stages; each invoked once for readability"
)]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use edge_toolkit::ports::Services;
use et_path::relative_path_from;
use fs_err as fs;
use serde::Deserialize;

mod deployment_types;
mod error;
mod hub_ws_url;
mod input;
mod module_package_json;
mod scenario_password;

// `pub` here means "reachable from the binary or from `tests/`", and nothing else.
// This crate is a command line tool that happens to be split into a lib target so integration tests can drive
// it; no consumer outside this directory builds on it, and nothing exported is a promise. Everything the
// generators share among themselves is `pub(crate)`, so what remains below is the whole of the surface anyone
// could depend on -- short enough to read, which is what makes an accidental addition to it visible.
pub use self::deployment_types::{ScenarioModules, docker_image_module_paths, scenario_module_paths};
pub(crate) use self::deployment_types::{
    generate_docker_compose_deployment, generate_k3s_deployment, generate_mise_deployment, generate_scenario_image,
};
pub use self::error::CliError;
pub(crate) use self::hub_ws_url::HUB_SERVICE;
pub use self::hub_ws_url::{hub_service_ws_url, hub_ws_url};
pub(crate) use self::input::running_in_this_repository;
pub use self::input::{
    ArtifactSource, ClusterInput, DEFAULT_CLUSTER_NAME, OutputType, infer_artifact_source,
    manifest_declares_this_repository,
};
pub use self::module_package_json::generate_module_package_json;
pub(crate) use self::scenario_password::{scenario_password, scenario_seed};

fn generated_output_files(output_types: &[OutputType]) -> Vec<&'static str> {
    let mut files = Vec::new();
    for output_type in output_types {
        files.push(output_type.output_file_name());
    }
    files
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DeploymentSummary {
    pub cluster_name: String,
    pub agent_templates: usize,
    pub module_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RegeneratedScenario {
    pub input_file: PathBuf,
    pub output_dir: PathBuf,
    pub summary: DeploymentSummary,
}

#[derive(Debug, Clone, Default, Deserialize)]
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
/// The deployment generators need this to tell apart the two provisioning routes: a repo directory can be
/// copied straight out of the Docker build context, whereas a mise-staged package exists only in the tool's
/// install dir and has to be installed before it can be staged.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) enum ModuleSource {
    /// A directory in this repository, as a path relative to the repository root.
    Repo(String),
    /// A package staged by a mise tool.
    ///
    /// Holds the backend-qualified tool id plus the published package name to locate beneath its install
    /// directory. The name rather than a path because the npm backend has no single layout: a package lands
    /// under `lib/node_modules/`, `node_modules/`, or an aube virtual store keyed by a content hash,
    /// depending on backend and platform, so the directory has to be found rather than assumed.
    MiseTool { tool: String, package: String },
}

/// Where the hub serves pyodide from, whichever distribution was selected.
const PYODIDE_DOCKER_PATH: &str = "/app/node_modules/pyodide";

/// Name of the generated env file that carries the scenario's derived credential.
///
/// The password is derived from the scenario input so a deployment is reproducible from it, which used to mean
/// writing the literal into `mise.toml` and `compose.yaml` -- both committed under `verification/`, where every
/// secret scanner duly found it. Collecting it into one file keeps the deployment reproducible while leaving
/// the rest of the generated output free of anything a scanner reads as a credential.
///
/// Whether that one file is committed depends on where it was generated, and [`write_deployment_gitignore`]
/// decides: under `verification/` it is a fixture the drift check reads, and anywhere else it is a real
/// credential that gets an ignore file written beside it.
pub(crate) const SECRETS_ENV_FILE: &str = "secrets.env";

/// Account the collector is created with, and the one the hub authenticates its OTLP exports as.
///
/// One constant because the two are the same account seen from either end: the collector is created with it as
/// `ZO_ROOT_USER_EMAIL` and the hub presents it as `OTLP_AUTH_USERNAME`, so a deployment where they disagree
/// comes up healthy and then rejects every export. Written in two files before this existed, with nothing
/// checking that they matched.
pub const COLLECTOR_USERNAME: &str = "root@example.com";

/// The collector's non-secret settings, which every generated deployment carries rather than sourcing.
///
/// Three formats render this -- a `ConfigMap`, a compose `environment:` block, a `docker run` flag list -- so
/// it is one list and a setting cannot reach some deployments and not others. Reading it from a file in this
/// repository instead is what made a generated deployment unable to leave the tree, for the sake of two values
/// neither secret nor scenario-specific.
///
/// Two things are deliberately absent. The root password is derived per scenario and reaches each format from
/// the generated env file. `ZO_DATA_DIR` is per format rather than shared, because it only means anything
/// alongside the storage that format declares -- a named volume, a claim, or nothing at all.
pub(crate) const COLLECTOR_SETTINGS: [(&str, &str); 2] =
    [("RUST_LOG", "warn"), ("ZO_ROOT_USER_EMAIL", COLLECTOR_USERNAME)];

/// Registry path the repository's own images are published under.
///
/// A runner image is the same for every deployment -- nothing in one varies by scenario -- so a scenario that
/// asks for published images names it here and the cluster pulls it, leaving the node with nothing to build or
/// import. The hub image is published alongside them but no manifest names it: it is the base a scenario image
/// is layered onto, so it reaches a deployment as a build context rather than as something a pod runs.
pub(crate) const IMAGE_REGISTRY: &str = et_org::IMAGE_REGISTRY;

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

#[derive(Debug, Clone)]
#[non_exhaustive]
pub(crate) struct ModuleRegistryEntry {
    pub mise_path: String,
    pub docker_path: String,
    pub dependencies: BTreeSet<String>,
    pub source: ModuleSource,
    /// The module's published package name, as `pkg/package.json` declares it.
    ///
    /// This is what a runner has to be told: `RUNNER_MODULE` is resolved against the names the hub serves
    /// modules under, which is the package name (`et-ws-math1`) and not the directory a scenario names it by
    /// (`math1`). `None` for a mise-staged package, which is already keyed by its published name.
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
    let cluster: ClusterInput = serde_yaml::from_slice(&content)?;
    validate_cluster_name(&cluster.cluster_name)?;

    Ok((cluster, scenario_seed(&content)))
}

/// Longest `cluster_name` that still leaves a valid Kubernetes namespace after the `et-` prefix.
///
/// A namespace is an RFC 1123 label, which the API server caps at 63 characters.
const CLUSTER_NAME_MAX: usize = 60;

/// Reject a `cluster_name` that is not an RFC 1123 label.
///
/// The name reaches three renderers that each read it as trusted text: it is interpolated into generated
/// comments, into shell commands in the generated README, and into Kubernetes object names. The comment case is
/// an instruction-injection vector on its own -- a name carrying a newline closes the scenario Dockerfile's
/// `# AUTO-GENERATED by ...` comment and everything after it is read by `BuildKit` as further instructions.
/// The README case is the same hazard against a shell, where `;` or a backtick would end the `scenario=<name>`
/// assignment and start a command. Rather than escape per renderer, the name is held to the narrowest alphabet
/// any of them needs, which is the one Kubernetes already demands of a namespace: lowercase alphanumerics and
/// `-`, starting and ending alphanumeric. That leaves nothing to escape anywhere, and it turns a name the API
/// server would have rejected at `kubectl apply` into an error at generation time.
fn validate_cluster_name(name: &str) -> Result<(), CliError> {
    let invalid = |reason: &str| {
        Err(CliError::InvalidClusterName {
            name: name.to_owned(),
            reason: reason.to_owned(),
        })
    };
    if name.is_empty() {
        return invalid("it is empty");
    }
    if name.len() > CLUSTER_NAME_MAX {
        return invalid(&format!("it is longer than {CLUSTER_NAME_MAX} characters"));
    }
    if let Some(bad) = name
        .chars()
        .find(|found| !(found.is_ascii_lowercase() || found.is_ascii_digit() || *found == '-'))
    {
        return invalid(&format!("it contains {bad:?}"));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return invalid("it starts or ends with '-'");
    }
    Ok(())
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
        let output_types = output_type.as_ref().map_or(OutputType::ALL, std::slice::from_ref);

        generate_deployment_outputs(&cluster, &output_dir, output_types, seed)?;
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

fn generate_deployment_outputs(
    cluster: &ClusterInput,
    output_dir: &Path,
    output_types: &[OutputType],
    seed: u64,
) -> Result<(), CliError> {
    if !output_dir.exists() {
        fs::create_dir_all(output_dir)?;
    }

    // One password per scenario, shared by both deployment formats.
    // OpenObserve and the ws-server have to agree on it: the server authenticates its OTLP exports against the
    // same root credentials the collector was started with.
    let password = scenario_password(seed);
    fs::write(output_dir.join(SECRETS_ENV_FILE), secrets_env(&password))?;
    write_deployment_gitignore(output_dir)?;
    for output_type in output_types {
        match output_type {
            OutputType::Mise => generate_mise_deployment(cluster, output_dir)?,
            OutputType::DockerCompose => {
                generate_docker_compose_deployment(cluster, output_dir)?;
                generate_scenario_image(cluster, output_dir)?;
            }
            // The scenario image is emitted here too, not just for compose.
            // Both formats reference it: compose builds it as a service, and the manifests name it as the
            // hub's image with the README giving the `docker build` for it. Generating k3s alone without it
            // produced a README pointing at a Dockerfile that was never written.
            OutputType::K3s => {
                generate_k3s_deployment(cluster, output_dir)?;
                generate_scenario_image(cluster, output_dir)?;
            }
        }
    }

    let readme_path = output_dir.join("README.md");
    let module_names = cluster_module_names(cluster);
    let dockerfile = scenario_dockerfile_path(output_dir);
    fs::write(
        &readme_path,
        generated_readme(cluster, &module_names, output_types, &dockerfile),
    )?;

    Ok(())
}

/// The path the generated README tells a reader to build this scenario's image from.
///
/// Only the parent is rendered, so the command keeps naming the last segment through the `scenario` shell
/// variable it already sets rather than repeating the name. Joined from the path's own components rather than
/// displayed, because the result is committed: a `Display` of the same path writes `\` on Windows and `/`
/// everywhere else, which would make the file drift by platform and fail the check that holds it stable.
/// Regeneration passes a repository-relative path, which is what the command needs, since it runs from the
/// repository root.
///
/// A single-component output directory has a parent, and it is the empty path rather than `None` -- so this
/// cannot lean on `unwrap_or` and has to test the rendered parent. Prefixing an empty one would produce
/// `/$scenario/Dockerfile`, an absolute path to a directory nobody has.
#[must_use]
pub fn scenario_dockerfile_path(output_dir: &Path) -> String {
    let parent = output_dir
        .parent()
        .unwrap_or(output_dir)
        .iter()
        .map(|component| component.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    if parent.is_empty() {
        return "$scenario/Dockerfile".to_string();
    }
    format!("{parent}/$scenario/Dockerfile")
}

/// Keep a generated deployment's credential out of whatever repository it was generated into.
///
/// Written beside the file it covers rather than left to the operator, because the failure is silent and
/// permanent: a credential committed once stays in the history after it is deleted. A nested `.gitignore` is
/// what makes the deployment directory safe to drop anywhere, which is the point of generating it.
///
/// Not written inside this repository. Here the verification outputs are committed evidence -- their whole
/// purpose is to be diffed when a generator changes -- and the password is derived from an input this
/// repository also carries, so it is reproducible from what is already public rather than a secret the file
/// is keeping. An ignore file here would only hide them from the drift check that exists to read them.
fn write_deployment_gitignore(output_dir: &Path) -> Result<(), CliError> {
    if running_in_this_repository() {
        return Ok(());
    }
    let body = format!(
        concat!(
            "# Written by `et-cli` beside the credential it covers.\n",
            "# The password is derived from the scenario input, so regenerating this deployment rewrites it;\n",
            "# committing it would publish the credential of every deployment generated from that input.\n",
            "{file}\n",
        ),
        file = SECRETS_ENV_FILE
    );
    fs::write(output_dir.join(".gitignore"), body)?;
    Ok(())
}

/// Render the env file every deployment format reads the scenario credential from.
///
/// Two names for the one password because the services that share it read different variables: `OpenObserve`
/// takes `ZO_ROOT_USER_PASSWORD` as its root credential, and the ws-server authenticates its OTLP exports with
/// `OTLP_AUTH_PASSWORD`. Nothing else belongs here. The account those two authenticate as is not a secret and
/// each format states it outright, so keeping it in this file would have meant an uncommitted file standing
/// between a reader and a value there was never any reason to withhold -- and, in the Kubernetes case, a
/// `Secret` holding something that is not one.
///
/// Each value carries a `skipcq` pragma for `DeepSource SCT-A000`: the copies committed under
/// `verification/` are read as hardcoded credentials, and the path excludes that keep the rest of that tree
/// out of analysis do not reach a secrets scan. The pragma sits on the line above rather than at the end of
/// its own, because an env file has no inline comments: every consumer keeps what follows the `=` verbatim,
/// so a trailing marker would become part of the password.
fn secrets_env(password: &str) -> String {
    format!(
        concat!(
            "# Generated from the scenario input -- regenerating this deployment rewrites it.\n",
            "# skipcq: SCT-A000 -- derived from the scenario input, not an independently generated secret\n",
            "ZO_ROOT_USER_PASSWORD={password}\n",
            "# skipcq: SCT-A000 -- derived from the scenario input, not an independently generated secret\n",
            "OTLP_AUTH_PASSWORD={password}\n",
        ),
        password = password
    )
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

fn generated_readme(
    cluster: &ClusterInput,
    module_names: &[String],
    output_types: &[OutputType],
    dockerfile: &str,
) -> String {
    let module_summary = if module_names.is_empty() {
        "No workflow modules were selected in the scenario input.".to_string()
    } else {
        format!(
            "The scenario exposes these workflow modules: {}.",
            module_names.join(", ")
        )
    };

    let output_files = generated_output_files(output_types);
    let output_summary = if let [only_file] = output_files.as_slice() {
        format!(
            "This directory contains the generated `{only_file}` for the `{}` scenario.",
            cluster.cluster_name
        )
    } else {
        let output_files = output_files
            .iter()
            .map(|output_file| format!("`{output_file}`"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            concat!(
                "This directory contains generated deployment configs for the `{}` scenario.\n",
                "Files: {}.",
            ),
            cluster.cluster_name, output_files
        )
    };
    let run_instructions = output_types
        .iter()
        .map(|output_type| {
            generated_run_instructions(
                *output_type,
                &cluster.cluster_name,
                dockerfile,
                &runner_kinds(cluster),
                cluster.artifact_source,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        concat!(
            "# {name}\n\n",
            "{output_summary}\n\n",
            "{module_summary}\n\n",
            "{artifact_note}",
            "{secrets_note}\n\n",
            "{run_instructions}",
        ),
        name = cluster.cluster_name,
        artifact_note = artifact_source_note(cluster.artifact_source),
        output_summary = output_summary,
        module_summary = module_summary,
        secrets_note = secrets_note(),
        run_instructions = run_instructions,
    )
}

/// State where this scenario's artifacts come from, for a scenario that does not build them.
///
/// Above the run sections rather than inside one, because it is true of every way this scenario starts: the
/// `mise` deployment runs released binaries, and the compose and Kubernetes ones run published images. Said
/// once per run mode it would be three copies of one fact, and said inside `mise` alone it would read as a
/// property of that mode. A local scenario says nothing here -- building what it runs is the unremarkable
/// case, and the run sections already show the builds.
const fn artifact_source_note(artifacts: ArtifactSource) -> &'static str {
    if matches!(artifacts, ArtifactSource::Published) {
        return concat!(
            "This scenario is rendered against released artifacts rather than builds of this repository:\n",
            "the images it names are published, and the binaries it runs are the released ones. Only the\n",
            "image carrying its own module set is still built locally, since no release can hold a module\n",
            "set particular to one deployment.\n\n"
        );
    }
    ""
}

/// Explain the credential file, since a deployment that reaches a new machine without it fails obscurely.
///
/// Worth saying out loud because its absence is silent: `mise` skips an `_.file` it cannot find without a
/// warning, and Docker Compose treats a missing `env_file` the same way, so a copy without it starts a
/// collector with no root password rather than failing.
fn secrets_note() -> String {
    format!(
        concat!(
            "`{file}` holds the scenario's derived OpenObserve and OTLP credentials. It is derived from the\n",
            "scenario input, so regenerating this deployment rewrites it; if it is missing, regenerate before\n",
            "starting the stack. A deployment generated outside the repository is written with a `.gitignore`\n",
            "covering it, so its credential is not committed by whatever repository it lands in.",
        ),
        file = SECRETS_ENV_FILE
    )
}

/// The distinct runner kinds a cluster names, in the order they first appear.
///
/// Deduplicated because a scenario with two agents on the same runner needs its image built once, and ordered
/// by first appearance rather than sorted so the generated instructions read in the order the input declares.
fn runner_kinds(cluster: &ClusterInput) -> Vec<String> {
    let mut kinds: Vec<String> = Vec::new();
    for agent in &cluster.agents {
        let Some(kind) = agent.runner.as_deref().map(str::trim).filter(|kind| !kind.is_empty()) else {
            continue;
        };
        if !kinds.iter().any(|seen| seen == kind) {
            kinds.push(kind.to_string());
        }
    }
    kinds
}

/// Render the step that installs a published deployment's binaries, and nothing at all for a local one.
///
/// A local deployment compiles what it runs from the working tree, so `mise run` is the whole of it. A
/// published one declares its binaries as `cargo:` tools, and `task.run_auto_install` is off, so without this
/// step the first task dies on a command it cannot find rather than fetching it. Why the binaries are released
/// rather than built is said once, above the run sections, because it is true of every mode.
const fn mise_install_note(artifacts: ArtifactSource) -> &'static str {
    if matches!(artifacts, ArtifactSource::Published) {
        return concat!(
            "Fetch the binaries the tasks below name before the first run:\n\n",
            "```bash\n",
            "mise install\n",
            "```\n\n"
        );
    }
    ""
}

fn generated_run_instructions(
    output_type: OutputType,
    cluster_name: &str,
    dockerfile: &str,
    runners: &[String],
    artifacts: ArtifactSource,
) -> String {
    match output_type {
        OutputType::Mise => format!(
            concat!(
                "## Run With Mise\n\n",
                "{install}",
                "From this directory, start the scenario with:\n\n",
                "```bash\n",
                "mise run generated-scenario\n",
                "```\n\n",
                "That task starts both OpenObserve and `ws-server` for this scenario.\n\n",
                "### Open The OpenObserve UI\n\n",
                "From this directory, open the OpenObserve UI with:\n\n",
                "```bash\n",
                "mise run open-o2\n",
                "```\n"
            ),
            install = mise_install_note(artifacts)
        ),
        OutputType::DockerCompose => format!(
            concat!(
                "## Run With Docker Compose\n\n",
                "From this directory, start the scenario with:\n\n",
                "```bash\n",
                "docker compose up --build\n",
                "```\n\n",
                "{hub}",
                "`ws-server` runs with host networking so it advertises the same LAN IP as the `mise` deployment.\n\n",
                "### Open The UIs\n\n",
                "OpenObserve is available at <http://localhost:5080/>.\n",
                "`ws-server` is available at <http://localhost:8080/> and <https://localhost:8443/>.\n\n",
                "Stop the scenario with:\n\n",
                "```bash\n",
                "docker compose down\n",
                "```\n"
            ),
            hub = compose_hub_note(artifacts)
        ),
        OutputType::K3s => k3s_run_instructions(cluster_name, dockerfile, runners, artifacts),
    }
}

/// Explain where the module-less hub image the scenario layers onto comes from.
///
/// The two answers are structurally different rather than differently worded: a local scenario declares a
/// build-only service for the hub and takes that service as the named build context, so `docker compose up`
/// builds two images; a published one points the context straight at the released image and builds one. A
/// reader who does not know which shape they have is reading a `compose.yaml` with a service in it they
/// cannot account for, or missing one the other scenarios have.
const fn compose_hub_note(artifacts: ArtifactSource) -> &'static str {
    if matches!(artifacts, ArtifactSource::Published) {
        return concat!(
            "The compose stack starts OpenObserve and builds one image: the `Dockerfile` in this directory,\n",
            "which stages this scenario's modules onto the released hub image it names as a build context.\n"
        );
    }
    concat!(
        "The compose stack starts OpenObserve and builds `ws-server` in two layers: the module-less hub\n",
        "image from the repository's `services/ws-server/Dockerfile`, then the `Dockerfile` in this\n",
        "directory, which stages this scenario's modules onto it. The hub is build-only and never runs as a\n",
        "container of its own.\n"
    )
}

/// Render the section covering each runner image the scenario's manifests name.
///
/// Commands for a locally sourced scenario and a plain list for a published one, because that is the
/// difference the reader has to act on: in the first case a runner image the node does not have leaves its pod
/// in `ErrImagePull` and nothing in the deployment builds it, and in the second the cluster fetches it and
/// there is nothing to run at all. The published case still names the refs, since a reader who just built the
/// scenario image by hand will otherwise go looking for the step that produces these. A scenario whose agents
/// are all browser-side names no runner image, so the whole section including its heading sentence is omitted
/// rather than left as an empty code fence.
fn runner_image_note(runners: &[String], images: ArtifactSource) -> String {
    if runners.is_empty() {
        return String::default();
    }
    if matches!(images, ArtifactSource::Published) {
        let mut refs = String::default();
        for kind in runners {
            let _write_result = writeln!(refs, "- `{IMAGE_REGISTRY}/et-ws-{kind}-runner:latest`");
        }
        return format!(
            concat!(
                "The cluster pulls this scenario's runner images, so nothing has to be built or imported for\n",
                "them:\n\n",
                "{refs}\n"
            ),
            refs = refs
        );
    }
    let mut commands = String::default();
    for kind in runners {
        let image = format!("et-ws-{kind}-runner:latest");
        let _write_result = writeln!(
            commands,
            "docker build -t {image} -f services/ws-{kind}-runner/Dockerfile ."
        );
        let _write_result = writeln!(commands, "docker save {image} | sudo k3s ctr images import -");
    }
    format!(
        concat!(
            "Then this scenario's runner images, which need no build context of their own:\n\n",
            "```bash\n",
            "{commands}",
            "```\n\n"
        ),
        commands = commands
    )
}

/// Render the paragraph and the hub reference that differ between the two image sources.
///
/// Kept apart from the shell below rather than templating two whole sections, because everything else about
/// producing the scenario image is the same either way: only what supplies `FROM hub`, and whether that hub is
/// a build of its own, actually change.
fn k3s_image_preamble(images: ArtifactSource) -> (&'static str, String, &'static str) {
    if matches!(images, ArtifactSource::Published) {
        return (
            concat!(
                "The manifests reference images by name and never build them, so this scenario's own image has\n",
                "to reach the node first. It layers its modules onto the module-less hub image and takes that\n",
                "hub as a _named build context_ rather than building it, so a plain `docker build` of the\n",
                "scenario Dockerfile fails on `FROM hub` -- pointing the context at the published hub is what\n",
                "supplies it. From the repository root:\n\n"
            ),
            format!("{IMAGE_REGISTRY}/et-ws-server:latest"),
            "",
        );
    }
    (
        concat!(
            "The manifests reference images by name and never build them, so build and import each one first.\n",
            "This scenario's image layers its modules onto the module-less hub image and takes that hub as a\n",
            "_named build context_ rather than building it, so the hub has to exist first -- a plain\n",
            "`docker build` of the scenario Dockerfile fails on `FROM hub`. From the repository root:\n\n"
        ),
        "et-ws-server-hub:latest".to_string(),
        "docker build -t \"$hub\" -f services/ws-server/Dockerfile .\n",
    )
}

/// Render the k3s half of the generated README.
///
/// Longer than the other two because a Kubernetes deployment needs two things done before `kubectl apply`
/// that neither `mise` nor compose does: the images have to exist on the node, since manifests reference
/// images rather than building them, and the credential has to be loaded as a `Secret`, since it is the one
/// generated file the repository does not carry.
fn k3s_run_instructions(cluster_name: &str, dockerfile: &str, runners: &[String], images: ArtifactSource) -> String {
    let (preamble, hub, hub_build) = k3s_image_preamble(images);
    format!(
        concat!(
            "## Run With k3s\n\n",
            "{preamble}",
            "```bash\n",
            "scenario={name}\n",
            "hub={hub}\n",
            "image=\"et-ws-server-$scenario:latest\"\n",
            "dockerfile=\"{dockerfile}\"\n",
            "{hub_build}",
            "docker build --build-context \"hub=docker-image://$hub\" -t \"$image\" -f \"$dockerfile\" .\n",
            "docker save \"$image\" | sudo k3s ctr images import -\n",
            "```\n\n",
            "{runner_note}",
            "### Load The Credential\n\n",
            "The credential reaches the pods as a `Secret` created from `{file}`, rather than written into\n",
            "`k3s.yaml` where it would be committed alongside the manifests. From this directory:\n\n",
            "```bash\n",
            "ns=et-{name}\n",
            "kubectl create namespace \"$ns\" --save-config\n",
            "kubectl create secret generic \"$ns-secrets\" --from-env-file={file} -n \"$ns\"\n",
            "```\n\n",
            "### Apply\n\n",
            "```bash\n",
            "kubectl apply -f k3s.yaml\n",
            "```\n\n",
            "The runners exit and restart until the hub reports ready, so `CrashLoopBackOff` while the hub\n",
            "starts is expected here rather than a fault. Watch it settle with:\n\n",
            "```bash\n",
            "kubectl get pods -n \"$ns\" --watch\n",
            "```\n"
        ),
        dockerfile = dockerfile,
        file = SECRETS_ENV_FILE,
        hub = hub,
        hub_build = hub_build,
        name = cluster_name,
        preamble = preamble,
        runner_note = runner_image_note(runners, images)
    )
}

#[must_use]
pub(crate) fn module_registry(project_root: &Path, ws_server_dir: &Path) -> BTreeMap<String, ModuleRegistryEntry> {
    let mut registry = BTreeMap::new();

    register_modules_under(
        &mut registry,
        &project_root.join("services/ws-modules"),
        ws_server_dir,
        "/app/services/ws-modules",
    );
    register_modules_under(
        &mut registry,
        &project_root.join("data/model-modules"),
        ws_server_dir,
        "/app/data/model-modules",
    );
    // Generated Python ws-modules: each generated/python-{ws,rest}/ holds
    // its own pkg/package.json after `mise run build-et-{ws,rest-client}-
    // wheel`. They're listed individually because the parent `generated/`
    // also contains non-module artifacts (rust-rest, dart-ws, zig-rest,
    // specs, docs).
    register_module_at(
        &mut registry,
        &project_root.join("generated/python-ws"),
        ws_server_dir,
        "/app/generated/python-ws",
    );
    register_module_at(
        &mut registry,
        &project_root.join("generated/python-rest"),
        ws_server_dir,
        "/app/generated/python-rest",
    );

    // The two the hub serves whatever the scenario asks for: its own page, and the agent that page loads.
    // Registered like any other module rather than prepended as bare paths by each generator, so the
    // dependencies they declare are resolved too. `static` names the runtimes its page pulls at boot, and a
    // deployment that omits them serves a page whose first import 404s.
    register_module_at(
        &mut registry,
        &project_root.join("services/ws-server/static"),
        ws_server_dir,
        "/app/services/ws-server/static",
    );
    register_module_at(
        &mut registry,
        &project_root.join("services/ws-wasm-agent"),
        ws_server_dir,
        "/app/services/ws-wasm-agent",
    );

    register_external_module(
        &mut registry,
        "onnxruntime-web",
        "npm:onnxruntime-web",
        "/app/node_modules/onnxruntime-web",
    );
    // The GPU utilisation overlay on the hub's page, declared by `static` alongside onnxruntime-web.
    register_external_module(&mut registry, "stats-gl", "npm:stats-gl", "/app/node_modules/stats-gl");
    // Registered as the full distribution, which `resolve_cluster_modules` narrows to the much smaller npm
    // package for a cluster whose modules never call `micropip.install`. The full one comes from a GitHub
    // release tarball that mise's http backend extracts flat, so its install dir is itself the module directory.
    register_external_module(&mut registry, "pyodide", "http:pyodide", PYODIDE_DOCKER_PATH);

    registry
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
/// Used for modules that don't live under `services/ws-modules/` --
/// currently the generated python clients under `generated/`.
fn register_module_at(
    registry: &mut BTreeMap<String, ModuleRegistryEntry>,
    module_path: &Path,
    ws_server_dir: &Path,
    docker_path: &str,
) {
    let Some(directory_name) = module_path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    register_module(registry, module_path, directory_name, ws_server_dir, docker_path);
}

/// Registry scope this project's own module packages carry, and which the hub drops when naming a module.
pub(crate) const MODULE_SCOPE: &str = et_org::NPM_SCOPE;

fn register_module(
    registry: &mut BTreeMap<String, ModuleRegistryEntry>,
    module_path: &Path,
    directory_name: &str,
    ws_server_dir: &Path,
    docker_path: &str,
) {
    let package = module_package_json(module_path);
    // The docker path is always the repo-relative path under `/app`, which is where the hub image roots its
    // module scan, so stripping that prefix recovers the path to copy out of the build context.
    let repo_path = docker_path.strip_prefix("/app/").unwrap_or(docker_path).to_string();
    // The name a module is served, resolved and referred to by is the one its published `package.json`
    // declares, scope and all -- and it has to be that name whether or not `pkg/` has been built, because
    // the lanes that generate a deployment build no modules. A generated manifest already carries the scope;
    // the source manifest standing in for it when `pkg/` is absent (`Cargo.toml`, `pyproject.toml`) names the
    // crate unscoped. So the rule that scopes a dependency scopes the module's own name too, which leaves an
    // already-scoped one untouched and keeps both sides of a dependency edge spelling the same key.
    let served_name = package
        .as_ref()
        .and_then(|package| package.name.clone())
        .map(|name| module_package_json::scoped_dependency_name(&name));
    let entry = ModuleRegistryEntry {
        mise_path: relative_path_from(ws_server_dir, module_path),
        docker_path: docker_path.to_string(),
        // Through the same scoping the generator applies when it writes a `package.json`, because the two
        // have to name the same module. A source manifest declares a dependency the way it declares its own
        // crate -- unscoped -- and publishing scopes both; reading one side raw would leave a dependency
        // naming something the registry has no key for.
        dependencies: package
            .as_ref()
            .map(|package| {
                package
                    .dependencies
                    .keys()
                    .map(|name| module_package_json::scoped_dependency_name(name))
                    .collect()
            })
            .unwrap_or_default(),
        source: ModuleSource::Repo(repo_path),
        package_name: served_name.clone(),
    };

    let _previous: Option<ModuleRegistryEntry> = registry.insert(directory_name.to_string(), entry.clone());
    if let Some(served_name) = served_name {
        let _previous: Option<ModuleRegistryEntry> = registry.insert(served_name, entry);
    }
}

/// Register a package that mise stages outside the repository, keyed by its published package name.
///
/// The mise path is a shell substitution rather than a literal, because where a tool's install dir keeps the
/// package is not knowable when the deployment is generated. An archive-backed `http:` tool extracts flat, so
/// `mise where` is already the answer; the npm backend spreads packages across several layouts that differ by
/// platform, so that case defers to `et-cli npm-module-path`, which resolves it through the same code the
/// ws-server uses to find these packages itself.
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
/// Separate from registration so the pyodide swap can rebuild an entry for a different tool without restating
/// how a mise path is spelled.
fn external_module_entry(package_name: &str, tool: &str, docker_path: &str) -> ModuleRegistryEntry {
    // Resolved at run time, because where mise's npm backend puts a package varies by backend and platform.
    // A local deployment names the packages it wants rather than asking the hub to serve everything this
    // config staged: the repository's own tools table mixes modules with development tooling, so serving all
    // of it would serve things that are not modules at all. An archive-backed tool
    // extracts flat, making its install directory the module directory, which `mise where` answers outright.
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
/// A module is registered under both its directory name and its `package.json` name, so the same entry is
/// reachable by two keys; de-duplicating on the docker path collapses those without disturbing the order the
/// generated files depend on.
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

pub(crate) fn resolve_module_paths<F>(
    registry: &BTreeMap<String, ModuleRegistryEntry>,
    module_names: &[String],
    path_for: F,
) -> Result<Vec<String>, CliError>
where
    F: Fn(&ModuleRegistryEntry) -> String,
{
    Ok(resolve_cluster_modules(registry, module_names)?
        .iter()
        .map(path_for)
        .collect())
}

/// Resolve the cluster's modules to the docker path each is served from and how it is provisioned.
pub(crate) fn resolve_module_sources(
    registry: &BTreeMap<String, ModuleRegistryEntry>,
    module_names: &[String],
) -> Result<Vec<(String, ModuleSource)>, CliError> {
    Ok(resolve_cluster_modules(registry, module_names)?
        .into_iter()
        .map(|entry| (entry.docker_path, entry.source))
        .collect())
}

/// Resolve a cluster's modules, sized to what those modules actually need.
///
/// Everything is taken from the registry as-is except pyodide, whose distribution depends on the cluster: the
/// registry cannot decide that, because pyodide arrives as a dependency of whichever Python modules the cluster
/// happens to declare.
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
/// Decided from the module's served `pkg/`, which is the code the browser actually runs, rather than from its
/// Python sources: the `micropip.install` calls live in each Python module's JS loader shim.
fn module_installs_wheels(project_root: &Path, entry: &ModuleRegistryEntry) -> bool {
    let ModuleSource::Repo(repo_path) = &entry.source else {
        return false;
    };
    let Ok(files) = fs::read_dir(project_root.join(repo_path).join("pkg")) else {
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
/// Defers to the resolver the ws-server itself uses, which is the only place that knows the layouts mise's npm
/// backend produces. Generated deployments call back into this rather than embedding a path, because the layout
/// differs per platform and backend and so cannot be decided when the deployment is generated.
pub fn npm_module_path(package: &str) -> Result<PathBuf, CliError> {
    edge_toolkit::config::mise_npm_package_path(package)
        .ok_or_else(|| CliError::UnresolvedNpmModule(package.to_string()))
}

/// Environment a runner gets from the deployment rather than from the scenario, and so cannot be overridden.
pub(crate) const DERIVED_RUNNER_ENV: [&str; 2] = ["RUNNER_MODULE", "WS_SERVER_URL"];

/// One runner process the generated deployment starts, resolved from an agent that names a `runner:`.
#[derive(Debug, Clone)]
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
}

/// Runner kinds a scenario may name, mapped to the crate that runs them.
///
/// All three share one deployment shape, which is what lets one generator serve them: each takes the module's
/// published name in `RUNNER_MODULE`, fetches it from the hub named by `WS_SERVER_URL`, and builds from
/// `services/ws-<kind>-runner/Dockerfile`. Nothing else distinguishes a runner here, so a fourth is this line
/// plus an image. Rejecting a kind by name is what stops a scenario from asking for one and silently getting
/// nothing.
pub(crate) const SUPPORTED_RUNNERS: [(&str, &str); 3] = [
    ("pyo3", "et-ws-pyo3-runner"),
    ("wasi", "et-ws-wasi-runner"),
    ("web", "et-ws-web-runner"),
];

/// Names the generated deployment already uses for its own tasks, services and aliases.
///
/// A runner is named after the agent that declares it, and both generators key on that name: mise inserts each
/// task into a table and compose writes each service as a mapping key. Either way a collision replaces rather
/// than reports -- an agent called `ws-server` would quietly take the hub's place, and the deployment would come
/// up missing the thing it was meant to talk to. Rejecting the name is the only way that surfaces.
pub(crate) const RESERVED_RUNNER_NAMES: [&str; 6] = [
    "generated-scenario",
    "o2",
    "open-o2",
    "openobserve",
    "ws-server",
    "ws-server-hub",
];

/// Resolve every agent that names a `runner:` into the processes the deployment has to start.
///
/// One process per resource rather than per agent, because a runner hosts exactly one module -- `RUNNER_MODULE`
/// is a single name. An agent with one resource (the usual shape) therefore keeps the agent's own name, and only
/// a multi-resource agent gets the resource suffixed, so the common case reads as the scenario wrote it.
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
        // A scenario cannot set the two the deployment derives for it.
        // `RUNNER_MODULE` and `WS_SERVER_URL` are what wire a runner to its module and its hub, and both are
        // computed from the rest of the input. Letting `env:` win would mean a scenario whose generated files
        // describe one deployment and whose runners join another; letting the derived value win would mean an
        // `env:` entry that is silently ignored. Neither is worth allowing, so it is an error to write one.
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
            runners.push(RunnerInstance {
                name,
                runner: runner.to_string(),
                module,
                // Every resource of a multi-resource agent gets its own runner process, and the agent's
                // environment describes the agent, so each of them carries it.
                env: agent.env.clone(),
            });
        }
    }
    Ok(runners)
}

/// Base HTTP URL a generated deployment reaches the hub on.
///
/// Every generator addresses the hub by its standard insecure port, so spelling the URL out in each of them
/// meant writing the same format string more than once. One definition here serves the compose services, the
/// mise tasks and whatever is added next, and it is the only place that has to change if the port moves.
#[must_use]
pub(crate) fn hub_http_base() -> String {
    format!("http://localhost:{}", Services::InsecureWebSocketServer.port())
}

/// Derive one runner's deployment-unique name, rejecting the two ways it can collide.
///
/// An agent with a single resource keeps its own name, so the common case reads as the scenario wrote it; only a
/// multi-resource agent gets the resource suffixed, because each resource becomes its own process.
///
/// Both checks exist because a collision would otherwise be silent rather than wrong-looking: mise inserts each
/// task into a table and compose writes each service as a mapping key, so a repeated name replaces what was there.
/// A scenario could lose its hub and only find out when the runners had nothing to talk to.
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
