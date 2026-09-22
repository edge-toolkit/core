use std::collections::BTreeMap;

use clap::ValueEnum;
use edge_toolkit::config::get_project_root;
use fs_err as fs;
use serde::{Deserialize, Serialize};

/// This repository, as its own workspace manifest states it.
///
/// Compared against rather than merely looked for, because the question it answers is identity: whether the
/// tree this process resolved is the one that builds these artifacts, not whether some Rust project is nearby.
const REPOSITORY_URL: &str = "https://github.com/edge-toolkit/core";

/// The deployment format a scenario is rendered into.
///
/// An enum rather than the string it is written as, because the set is closed and every consumer switches on
/// it: as a string, a misspelling reaches the generator, and the three writers of these files each have to
/// re-decide what an unrecognised one means. Deserialization rejects anything outside the set instead, naming
/// the alternatives, at the point the input is read.
#[expect(
    clippy::exhaustive_enums,
    reason = "OutputType enumerates the supported deployment formats; downstream code matches exhaustively"
)]
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum OutputType {
    #[default]
    Mise,
    #[serde(rename = "docker-compose", alias = "docker_compose")]
    DockerCompose,
    K3s,
}

impl OutputType {
    pub const ALL: &'static [Self] = &[Self::Mise, Self::DockerCompose, Self::K3s];

    #[must_use]
    pub const fn output_file_name(self) -> &'static str {
        match self {
            Self::Mise => "mise.toml",
            Self::DockerCompose => "compose.yaml",
            Self::K3s => "k3s.yaml",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ClusterInput {
    pub cluster_name: String,
    /// The format this scenario is rendered into unless `--output-type` overrides it, from `deployment_type:`.
    #[serde(default)]
    pub deployment_type: OutputType,
    /// Where the artifacts a generated deployment consumes come from, from `artifact_source:`.
    ///
    /// Stated, it is honoured as written, in both directions. Left out, it is settled while the input is read
    /// -- see [`infer_artifact_source`] -- so that by the time anything holds a `ClusterInput` the question is
    /// answered and there is no second, later notion of "unset" for a consumer to re-resolve.
    #[serde(default = "infer_artifact_source")]
    pub artifact_source: ArtifactSource,
    /// The agents this cluster runs, from `agents:`.
    ///
    /// Defaults to none, which is a cluster of the hub and its collector and nothing else. That is a real
    /// deployment rather than a degenerate one -- it serves the UI and accepts agents that connect to it --
    /// and it is what a scenario stating nothing but its name describes.
    #[serde(default)]
    pub agents: Vec<Agent>,
}

/// The name a cluster takes when its input does not choose one.
pub const DEFAULT_CLUSTER_NAME: &str = "default";

impl Default for ClusterInput {
    /// The smallest cluster that is still valid: named, no agents, rendered the way an unstated input is.
    ///
    /// Not derived, because a derived `cluster_name` is the empty string and an empty name is one of the
    /// things scenario validation exists to reject -- a `Default` that cannot be generated from would be a
    /// trap rather than a starting point. `artifact_source` is the enum's own default rather than the
    /// inferred one, because this is the value with nothing to infer from; reading an input is where a tree
    /// gets a say.
    fn default() -> Self {
        Self {
            cluster_name: DEFAULT_CLUSTER_NAME.to_string(),
            deployment_type: OutputType::default(),
            artifact_source: ArtifactSource::default(),
            agents: Vec::new(),
        }
    }
}

/// Which copy of this project's own artifacts a generated deployment addresses.
///
/// `Local` builds everything from the working tree: a Kubernetes manifest names image tags the deploying
/// machine has to produce and import into the node, and a `mise` deployment runs its binaries out of the cargo
/// workspace. `Published` addresses what has been released instead -- container images from the project's
/// registry, binaries from the crates.io releases -- so a deployment consumes the project rather than
/// rebuilding it. What a scenario cannot get either way is its own module set, which is built from the tree
/// because it is particular to that deployment.
///
/// `Published` is the default because it is the only one of the two that always works. Building from the
/// working tree needs a working tree, so `Local` is an answer available to almost nobody: everyone generating
/// a deployment for their own cluster has the releases and not this repository.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "lowercase")]
pub enum ArtifactSource {
    Local,
    #[default]
    Published,
}

/// Settle where an input that did not say gets its artifacts from.
///
/// `Published`, except where there is proof it need not be: inside this repository every artifact can be
/// built from the tree, and building what you are working on is the whole point of generating a deployment
/// there. Anywhere else the releases are the only thing that exists.
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
/// Two decisions turn on it, and they are the same question asked twice: whether a deployment can be built
/// from the tree, and whether a generated credential belongs to a committed fixture or to someone's real
/// deployment. A root with no readable manifest is not this repository, which is the same answer as a root
/// whose manifest belongs to something else, so the read failing needs no handling of its own.
#[must_use]
pub fn running_in_this_repository() -> bool {
    let manifest = get_project_root().join("Cargo.toml");
    fs::read_to_string(&manifest).is_ok_and(|text| manifest_declares_this_repository(&text))
}

/// Whether a workspace manifest is this repository's.
///
/// Split from the file read so the decision is a pure function of the text and can be exercised over every
/// shape a manifest takes -- another project's, one with no `[workspace.package]`, one that is not TOML at
/// all -- none of which is reachable through the caller, since the caller can only ever see whichever
/// manifest the running process happens to sit under. Public for that reason and no other.
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Agent {
    pub name: String,
    /// Runner that executes this agent's modules, from `runner:`.
    ///
    /// Unset means the modules are only served, for a browser to load and run itself -- which is what every
    /// deployment did before this field existed. Set, the generated deployment also starts a runner process per
    /// resource, so the cluster runs headless.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<String>,
    /// Extra environment for this agent's runner processes, from `env:`.
    ///
    /// Every deployment format has somewhere to put these -- a `mise` task's `[env]`, a compose service's
    /// `environment:`, a Kubernetes container's `env:` -- so a scenario that needs a runner configured says
    /// so once, here, rather than the operator editing three generated files that a regeneration overwrites.
    ///
    /// A `BTreeMap` so the rendering is ordered: the outputs are committed and diffed, and a hash map would
    /// reorder them between runs for no reason anyone could act on.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    pub resources: Vec<Resource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Resource {
    #[serde(rename = "type")]
    pub resource_type: String,
}
