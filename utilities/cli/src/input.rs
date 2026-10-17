use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::ValueEnum;
use edge_toolkit::config::get_project_root;
use fs_err as fs;
use serde::{Deserialize, Serialize};

use crate::limits::Limits;
use crate::schedule::Schedule;

/// This repository, as its own workspace manifest states it.
///
/// Compared against rather than merely looked for, because the question it answers is identity: whether the tree this
/// process resolved is the one that builds these artifacts, not whether some Rust project is nearby.
const REPOSITORY_URL: &str = et_org::REPOSITORY_URL;

/// The deployment format a scenario is rendered into.
///
/// An enum rather than the string it is written as, because the set is closed and every consumer switches on it: as
/// a string, a misspelling reaches the generator, and the three writers of these files each have to re-decide what an
/// unrecognised one means. Deserialization rejects anything outside the set instead, naming the alternatives, at the
/// point the input is read.
#[expect(
    clippy::exhaustive_enums,
    reason = "OutputType enumerates the supported deployment formats; downstream code matches exhaustively"
)]
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum OutputType {
    #[default]
    Mise,
    #[serde(rename = "docker-compose", alias = "docker_compose")]
    DockerCompose,
    K3s,
    Dekit,
    #[serde(rename = "process-compose", alias = "process_compose")]
    ProcessCompose,
}

impl OutputType {
    pub const ALL: &'static [Self] = &[
        Self::Mise,
        Self::DockerCompose,
        Self::K3s,
        Self::Dekit,
        Self::ProcessCompose,
    ];

    /// The file this format is written to, beside the others in the deployment directory.
    #[must_use]
    pub const fn output_file_name(self) -> &'static str {
        match self {
            Self::Mise => "mise.toml",
            Self::DockerCompose => "compose.yaml",
            Self::K3s => "k3s.yaml",
            Self::Dekit => "dekit.yaml",
            Self::ProcessCompose => "process-compose.yaml",
        }
    }

    /// The format as a scenario's `deployment_type:` and `--output-type` spell it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Mise => "mise",
            Self::DockerCompose => "docker-compose",
            Self::K3s => "k3s",
            Self::Dekit => "dekit",
            Self::ProcessCompose => "process-compose",
        }
    }

    /// Whether this format can start an agent on its `schedule:`.
    ///
    /// mise and dekit have no scheduler of their own, so a scheduled agent is refused rather than run continuously.
    #[must_use]
    pub const fn runs_schedules(self) -> bool {
        !matches!(self, Self::Mise | Self::Dekit)
    }

    /// Whether this format runs the tasks of a `mise.toml`, so needs what that file names installed first.
    #[must_use]
    pub const fn runs_mise_tasks(self) -> bool {
        matches!(self, Self::Mise | Self::Dekit | Self::ProcessCompose)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct ClusterInput {
    pub cluster_name: String,
    /// The format this scenario is rendered into unless `--output-type` overrides it, from `deployment_type:`.
    #[serde(default)]
    pub deployment_type: OutputType,
    /// Where the artifacts a generated deployment consumes come from, from `artifact_source:`.
    ///
    /// Stated, it is honoured as written, in both directions. Left out, it is settled while the input is read -- see
    /// [`infer_artifact_source`] -- so that by the time anything holds a `ClusterInput` the question is answered and
    /// there is no second, later notion of "unset" for a consumer to re-resolve.
    #[serde(default = "infer_artifact_source")]
    pub artifact_source: ArtifactSource,
    /// The agents this cluster runs, from `agents:`.
    ///
    /// Defaults to none, which is a cluster of the hub and its collector and nothing else. That is a real deployment
    /// rather than a degenerate one -- it serves the UI and accepts agents that connect to it -- and it is what a
    /// scenario stating nothing but its name describes.
    #[serde(default)]
    pub agents: Vec<Agent>,
    /// Module directories the scenario names itself, from `module_paths:`, alongside the ones the registry knows.
    ///
    /// Each entry is either a module directory itself or a parent of several, the same two shapes the hub's own
    /// `MODULES_PATHS` accepts. Written relative to the input file, so a scenario can name the modules it keeps beside
    /// it wherever the CLI is run from; reading the input resolves them to absolute paths, so nothing downstream has to
    /// know where the input was.
    ///
    /// These modules come from their directories whatever the artifact source: they belong to whoever wrote the
    /// scenario, so there is no release of them for a published deployment to stage instead.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub module_paths: Vec<PathBuf>,
    /// The machines the cluster spans, from `machines:`.
    ///
    /// Empty is a single-host cluster, which is every deployment's shape when the field is left out: everything runs
    /// on the one machine the deployment is started on and reaches the hub at `localhost`. Listed, the first machine
    /// is the server -- it runs the hub and the collector, whose storage is local to it -- and every agent runs on
    /// the machine that lists it, or on the server when none does. Each format splits along the same line: a k3s node
    /// label, a compose profile, a `machine-<name>` mise task and a dekit tag per machine, with a runner off the server
    /// reaching the hub at the server's `address`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub machines: Vec<Machine>,
    /// Settings for the processes every deployment runs whatever its agents, from `services:`.
    #[serde(default, skip_serializing_if = "Services::is_unset")]
    pub services: Services,
}

/// The processes every deployment runs alongside the agents' runners, each with its own settings.
///
/// A table of its own, and each process a table within it, so a setting a later version adds has a place to go without
/// a new top-level key per process and per setting.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Services {
    /// The telemetry collector, from `services.collector:`.
    #[serde(default, skip_serializing_if = "Service::is_unset")]
    pub collector: Service,
    /// The hub, from `services.hub:`.
    #[serde(default, skip_serializing_if = "Service::is_unset")]
    pub hub: Service,
}

impl Services {
    /// Whether neither service sets anything, which is how a scenario that predates `services:` reads.
    #[must_use]
    pub fn is_unset(&self) -> bool {
        *self == Self::default()
    }
}

/// One of the deployment's own processes and the settings the scenario gives it.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Service {
    /// Resource limits for the process, from `limits:`; the same fields an agent's `limits:` takes.
    #[serde(default, skip_serializing_if = "Limits::is_unset")]
    pub limits: Limits,
}

impl Service {
    /// Whether the scenario sets nothing for this service.
    #[must_use]
    pub fn is_unset(&self) -> bool {
        *self == Self::default()
    }
}

/// One machine of a multi-machine cluster, and the agents that run on it.
///
/// The agents are listed here rather than each agent naming its machine, so that an agent that names none has no third
/// state to resolve: it runs on the server, which is what an unlisted agent means.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct Machine {
    /// What the generated deployment calls the machine: its k3s node label, compose profile, mise task and dekit tag.
    pub name: String,
    /// The host name or IPv4 address the other machines reach this one at.
    pub address: String,
    /// Names of the agents that run on this machine.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<String>,
}

/// The name a cluster takes when its input does not choose one.
pub const DEFAULT_CLUSTER_NAME: &str = "default";

impl Default for ClusterInput {
    /// The smallest cluster that is still valid: named, no agents, rendered the way an unstated input is.
    ///
    /// Not derived, because a derived `cluster_name` is the empty string and an empty name is one of the things
    /// scenario validation exists to reject -- a `Default` that cannot be generated from would be a trap rather than
    /// a starting point. `artifact_source` is the enum's own default rather than the inferred one, because this is the
    /// value with nothing to infer from; reading an input is where a tree gets a say.
    fn default() -> Self {
        Self {
            cluster_name: DEFAULT_CLUSTER_NAME.to_string(),
            deployment_type: OutputType::default(),
            artifact_source: ArtifactSource::default(),
            agents: Vec::new(),
            module_paths: Vec::new(),
            machines: Vec::new(),
            services: Services::default(),
        }
    }
}

/// Which copy of this project's own artifacts a generated deployment addresses.
///
/// `Local` builds everything from the working tree: a Kubernetes manifest names image tags the deploying machine
/// has to produce and import into the node, and a `mise` deployment runs its binaries out of the cargo workspace.
/// `Published` addresses what has been released instead -- container images from the project's registry, binaries from
/// the crates.io releases -- so a deployment consumes the project rather than rebuilding it. What a scenario cannot
/// get either way is an image of its own module set, which no release can hold because the set is particular to that
/// deployment: it is built for the deployment, from the tree's modules or from their releases.
///
/// `Published` is the default because it is the only one of the two that always works. Building from the working tree
/// needs a working tree, so `Local` is an answer available to almost nobody: everyone generating a deployment for their
/// own cluster has the releases and not this repository.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "lowercase")]
pub enum ArtifactSource {
    Local,
    #[default]
    Published,
}

/// Settle where an input that did not say gets its artifacts from.
///
/// `Published`, except where there is proof it need not be: inside this repository every artifact can be built from the
/// tree, and building what you are working on is the whole point of generating a deployment there. Anywhere else the
/// releases are the only thing that exists.
#[must_use]
pub fn infer_artifact_source() -> ArtifactSource {
    if running_in_this_repository() {
        ArtifactSource::Local
    } else {
        ArtifactSource::default()
    }
}

/// Whether this process is running inside the repository that builds these artifacts.
///
/// Two decisions turn on it, and they are the same question asked twice: whether a deployment can be built from the
/// tree, and whether a generated credential belongs to a committed fixture or to someone's real deployment. A root with
/// no readable manifest is not this repository, which is the same answer as a root whose manifest belongs to something
/// else, so the read failing needs no handling of its own.
#[must_use]
pub fn running_in_this_repository() -> bool {
    let manifest = get_project_root().join("Cargo.toml");
    fs::read_to_string(&manifest).is_ok_and(|text| manifest_declares_this_repository(&text))
}

/// Whether a workspace manifest is this repository's.
///
/// Split from the file read so the decision is a pure function of the text and can be exercised over every shape a
/// manifest takes -- another project's, one with no `[workspace.package]`, one that is not TOML at all -- none of which
/// is reachable through the caller, since the caller can only ever see whichever manifest the running process happens
/// to sit under. Public for that reason and no other.
#[must_use]
pub fn manifest_declares_this_repository(manifest: &str) -> bool {
    let Ok(parsed) = manifest.parse::<toml::Table>() else {
        return false;
    };
    parsed
        .get("workspace")
        .and_then(|workspace| workspace.get("package"))
        .and_then(|package| package.get("repository"))
        .and_then(toml::Value::as_str)
        == Some(REPOSITORY_URL)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct Agent {
    pub name: String,
    /// Runner that executes this agent's modules, from `runner:`.
    ///
    /// Unset means the modules are only served, for a browser to load and run itself -- which is what every deployment
    /// did before this field existed. Set, the generated deployment also starts a runner process per resource, so the
    /// cluster runs headless.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<String>,
    /// Extra environment for this agent's runner processes, from `env:`.
    ///
    /// Every deployment format has somewhere to put these -- a `mise` task's `[env]`, a compose service's
    /// `environment:`, a Kubernetes container's `env:` -- so a scenario that needs a runner configured says so once,
    /// here, rather than the operator editing three generated files that a regeneration overwrites.
    ///
    /// A `BTreeMap` so the rendering is ordered: the outputs are committed and diffed, and a hash map would reorder
    /// them between runs for no reason anyone could act on.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Resource limits each of this agent's runner processes runs under, from `limits:`.
    #[serde(default, skip_serializing_if = "Limits::is_unset")]
    pub limits: Limits,
    /// When this agent's runners are started, from `schedule:`; unset runs them continuously.
    #[serde(default, skip_serializing_if = "Schedule::is_unset")]
    pub schedule: Schedule,
    pub resources: Vec<Resource>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct Resource {
    #[serde(rename = "type")]
    pub resource_type: String,
}
