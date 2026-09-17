//! Covers how a scenario input deserializes, and what each field answers when the input leaves it out.
//!
//! Every default here is load-bearing, because `verification/*/input/default.yaml` is an input that states
//! nothing but its name and whose committed output is whatever these produce. The one that most repays a
//! test is `artifact_source`, which is not a fixed default at all: it is settled against the tree the
//! process is running in, so the same input means "build it" in this repository and "pull it" anywhere else.
#![cfg(test)]

use et_cli::{
    ArtifactSource, ClusterInput, DEFAULT_CLUSTER_NAME, OutputType, infer_artifact_source,
    manifest_declares_this_repository,
};

/// A scenario stating only what has no default, which is what the `default` verification scenario is.
const ONLY_A_NAME: &str = "cluster_name: default\n";

/// A scenario with agents, for the fields that only mean something once there are some.
const WITH_AN_AGENT: &str = "
cluster_name: math1
agents:
  - name: math1-twin
    runner: web
    resources:
      - type: math1
";

fn parse(yaml: &str) -> ClusterInput {
    serde_yaml::from_str(yaml).unwrap()
}

#[test]
fn an_input_stating_only_a_name_takes_every_other_field_from_its_default() {
    let cluster = parse(ONLY_A_NAME);

    assert_eq!(cluster.cluster_name, DEFAULT_CLUSTER_NAME);
    assert_eq!(cluster.deployment_type, OutputType::Mise);
    assert!(cluster.agents.is_empty(), "a hub and its collector, and nothing else");
}

#[test]
fn the_default_cluster_input_is_one_that_could_be_generated_from() {
    // A derived `Default` would give it the empty name that scenario validation rejects, so the value would
    // exist only to be invalid. This is the same cluster `ONLY_A_NAME` parses to, apart from the source.
    let cluster = ClusterInput::default();

    assert_eq!(cluster.cluster_name, DEFAULT_CLUSTER_NAME);
    assert_eq!(cluster.deployment_type, OutputType::Mise);
    assert_eq!(cluster.artifact_source, ArtifactSource::Published);
    assert!(cluster.agents.is_empty());
}

#[test]
fn published_is_the_artifact_source_with_nothing_to_infer_from() {
    // The fallback for anywhere that cannot build these artifacts, which is everywhere but this repository.
    assert_eq!(ArtifactSource::default(), ArtifactSource::Published);
}

#[test]
fn an_unstated_artifact_source_infers_local_inside_this_repository() {
    // These tests run from a checkout, which is the proof the inference looks for. Were this to answer
    // `Published`, every scenario under `verification/local` would quietly start naming the registry.
    assert_eq!(infer_artifact_source(), ArtifactSource::Local);
    assert_eq!(parse(ONLY_A_NAME).artifact_source, ArtifactSource::Local);
}

#[test]
fn a_stated_artifact_source_is_honoured_over_what_the_tree_would_infer() {
    // In both directions, including a `published` stated here, where the tree could have supplied
    // everything: that input describes a deployment somewhere else.
    let local = parse(&format!("{ONLY_A_NAME}artifact_source: local\n"));
    let published = parse(&format!("{ONLY_A_NAME}artifact_source: published\n"));

    assert_eq!(local.artifact_source, ArtifactSource::Local);
    assert_eq!(published.artifact_source, ArtifactSource::Published);
}

#[test]
fn only_this_repositories_own_manifest_counts_as_proof() {
    // The cases the caller cannot reach, since it only ever sees the manifest it happens to sit under.
    let ours = "[workspace.package]\nrepository = \"https://github.com/edge-toolkit/core\"\n";
    assert!(manifest_declares_this_repository(ours));

    let theirs = "[workspace.package]\nrepository = \"https://github.com/edge-toolkit/research\"\n";
    assert!(
        !manifest_declares_this_repository(theirs),
        "another project's workspace"
    );
    assert!(
        !manifest_declares_this_repository("[workspace]\nmembers = []\n"),
        "a workspace that declares no package metadata"
    );
    assert!(
        !manifest_declares_this_repository("[package]\nname = \"solo\"\n"),
        "a manifest that is not a workspace at all"
    );
    assert!(
        !manifest_declares_this_repository("this is not toml { ["),
        "and something that does not parse"
    );
}

#[test]
fn every_deployment_type_spelling_deserializes_to_its_own_output_file() {
    for (spelling, expected, file) in [
        ("mise", OutputType::Mise, "mise.toml"),
        ("docker-compose", OutputType::DockerCompose, "compose.yaml"),
        ("docker_compose", OutputType::DockerCompose, "compose.yaml"),
        ("k3s", OutputType::K3s, "k3s.yaml"),
    ] {
        let cluster = parse(&format!("{ONLY_A_NAME}deployment_type: {spelling}\n"));

        assert_eq!(cluster.deployment_type, expected, "for {spelling}");
        assert_eq!(cluster.deployment_type.output_file_name(), file);
    }
    assert_eq!(OutputType::ALL.len(), 3, "and the list of them is complete");
}

#[test]
fn an_unknown_deployment_type_is_rejected() {
    // The reason it is an enum: as a string, a misspelling reached the generator instead of the reader.
    let parsed: Result<ClusterInput, _> = serde_yaml::from_str(&format!("{ONLY_A_NAME}deployment_type: yaml\n"));

    let rejected = parsed.unwrap_err().to_string();

    assert!(rejected.contains("yaml"), "names what was written: {rejected}");
    assert!(rejected.contains("mise"), "and what it could have been: {rejected}");
}

#[test]
fn an_unknown_artifact_source_is_rejected() {
    // Rather than falling back to a default, which would turn a typo into a deployment addressing the wrong
    // artifacts and report nothing at generation time.
    let parsed: Result<ClusterInput, _> = serde_yaml::from_str(&format!("{ONLY_A_NAME}artifact_source: registry\n"));

    let _rejected = parsed.unwrap_err();
}

#[test]
fn an_agent_keeps_its_runner_and_resources() {
    let cluster = parse(WITH_AN_AGENT);

    let [agent] = cluster.agents.as_slice() else {
        panic!("expected exactly one agent");
    };
    assert_eq!(agent.name, "math1-twin");
    assert_eq!(agent.runner.as_deref(), Some("web"));
    let [resource] = agent.resources.as_slice() else {
        panic!("expected exactly one resource");
    };
    assert_eq!(resource.resource_type, "math1");
}

#[test]
fn a_cluster_input_round_trips_through_yaml() {
    // Serialization is a public surface of its own: `et-cli` writes a scenario back out, and a `skip`
    // attribute on the wrong field would drop it silently rather than failing to compile.
    let cluster = parse(&format!("{WITH_AN_AGENT}artifact_source: published\n"));

    let rendered = serde_yaml::to_string(&cluster).unwrap();
    let reparsed = parse(&rendered);

    assert_eq!(reparsed.artifact_source, ArtifactSource::Published);
    assert_eq!(reparsed.deployment_type, cluster.deployment_type);
    assert_eq!(reparsed.cluster_name, cluster.cluster_name);
    assert_eq!(reparsed.agents.len(), cluster.agents.len());
}
