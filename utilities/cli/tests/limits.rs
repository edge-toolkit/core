//! Covers per-process resource limits: how `limits:` is read and validated, and that compose and k3s apply each one.
//!
//! The fixture limits the math1 twin and leaves the trigger unset, so each test sees both a limited process and one
//! that runs under the defaults alone. The parity test is the guard on the whole feature: every field the schema
//! accepts has to change what both container formats render.
#![cfg(test)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use et_cli::{ClusterInput, Cpus, Limits, Ulimit, generate_deployment, regenerate_verification};
use fs_err as fs;
use k8s_openapi::api::apps::v1::{DaemonSet, Deployment};
use k8s_openapi::api::core::v1::{ConfigMap, PodSpec};
use serde::Deserialize as _;
use tempfile::{TempDir, tempdir};

/// The math1 pair, with `{twin}` spliced in as the twin's `limits:` block and `{rest}` appended at the top level.
fn scenario(twin: &str, rest: &str) -> String {
    format!(
        concat!(
            "cluster_name: \"limited\"\n",
            "agents:\n",
            "  - name: \"math1-twin\"\n",
            "    runner: \"web\"\n",
            "{twin}",
            "    resources:\n",
            "      - type: \"math1\"\n",
            "  - name: \"math1-trigger\"\n",
            "    runner: \"web\"\n",
            "    resources:\n",
            "      - type: \"math1-sender\"\n",
            "{rest}"
        ),
        rest = rest,
        twin = twin,
    )
}

/// `fields`, one `key: value` per line, as the twin's `limits:` block.
fn twin_limits(fields: &str) -> String {
    fields.lines().fold("    limits:\n".to_owned(), |mut block, line| {
        let _write_result = writeln!(block, "      {line}");
        block
    })
}

/// Every limit that can be set together: the quota cap and the share weight rather than their alternatives.
const EVERY_LIMIT: &str = "blkio_weight: 300
cpu_period: 50000
cpu_quota: 75000
cpu_shares: 512
cpuset: \"0-3\"
memory: \"1.5 GiB\"
memory_reservation: \"256 MiB\"
memswap_limit: \"2 GiB\"
pids: 256
shm_size: \"128 MiB\"
ulimits:
  core: 0
  nofile:
    soft: 1024
    hard: 4096";

/// Every format generated from `input` into a fresh `verification/` tree, with the root kept alive.
fn generated(input: &str) -> (TempDir, PathBuf) {
    let test_root = tempdir().unwrap();
    let verification_root = test_root.path().join("verification");
    let input_dir = verification_root.join("local/input");
    fs::create_dir_all(&input_dir).unwrap();
    fs::write(input_dir.join("cluster.yaml"), input).unwrap();
    let _regenerated = regenerate_verification(&verification_root, None).unwrap();
    (test_root, verification_root.join("local/output/cluster"))
}

fn read(dir: &Path, file: &str) -> String {
    fs::read_to_string(dir.join(file)).unwrap()
}

fn compose_service(dir: &Path, name: &str) -> serde_yaml::Value {
    let compose: serde_yaml::Value = serde_yaml::from_str(&read(dir, "compose.yaml")).unwrap();
    compose["services"][name].clone()
}

/// Every document of `k3s.yaml` whose kind is `kind`.
fn documents<T>(dir: &Path, kind: &str) -> Vec<T>
where
    T: serde::de::DeserializeOwned,
{
    let manifest = read(dir, "k3s.yaml");
    serde_yaml::Deserializer::from_str(&manifest)
        .map(|document| serde_yaml::Value::deserialize(document).unwrap())
        .filter(|document| document["kind"] == kind)
        .map(|document| serde_yaml::from_value(document).unwrap())
        .collect()
}

/// The k3s `Deployment` called `name`.
fn deployment(dir: &Path, name: &str) -> Deployment {
    documents::<Deployment>(dir, "Deployment")
        .into_iter()
        .find(|deployment| deployment.metadata.name.as_deref() == Some(name))
        .unwrap()
}

/// The pod of the k3s `Deployment` called `name`.
fn pod(dir: &Path, name: &str) -> PodSpec {
    deployment(dir, name).spec.unwrap().template.spec.unwrap()
}

/// The annotations on the pods of the k3s `Deployment` called `name`, empty when there are none.
fn annotations(dir: &Path, name: &str) -> BTreeMap<String, String> {
    let template = deployment(dir, name).spec.unwrap().template;
    template.metadata.and_then(|meta| meta.annotations).unwrap_or_default()
}

/// The CPU and memory limits, then requests, of the container in `name`'s pod.
fn resources(dir: &Path, name: &str) -> [[String; 2]; 2] {
    let resources = pod(dir, name).containers[0].resources.clone().unwrap();
    [resources.limits.unwrap(), resources.requests.unwrap()].map(|map| [map["cpu"].0.clone(), map["memory"].0.clone()])
}

/// `pairs` as owned strings, the shape [`resources`] returns.
fn quantities(pairs: [[&str; 2]; 2]) -> [[String; 2]; 2] {
    pairs.map(|pair| pair.map(String::from))
}

/// The error generating a scenario with `twin` as the twin's limits produces.
fn error_for(twin: &str, rest: &str) -> String {
    let test_root = tempdir().unwrap();
    let input_file = test_root.path().join("cluster.yaml");
    fs::write(&input_file, scenario(twin, rest)).unwrap();
    generate_deployment(&input_file, &test_root.path().join("output"), None)
        .unwrap_err()
        .to_string()
}

fn limits(yaml: &str) -> Result<Limits, serde_yaml::Error> {
    serde_yaml::from_str(yaml)
}

/// The name of every field `Limits` accepts, read from serde's own refusal of one it does not.
///
/// `deny_unknown_fields` lists every accepted name in that error, so this is the schema itself rather than a copy of
/// it that a new field could be left out of.
#[expect(
    clippy::single_call_fn,
    reason = "the error-text parsing is its own step, kept apart from the parity test it feeds"
)]
fn schema_fields() -> Vec<String> {
    let error = limits("not_a_limit: 1").unwrap_err().to_string();
    let (_, expected) = error.split_once("expected one of ").unwrap();
    expected
        .split(", ")
        .map(|name| name.trim_matches(|character: char| !character.is_ascii_alphanumeric() && character != '_'))
        .map(String::from)
        .collect()
}

/// A top-level `services:` block giving `service` the limits `fields`, one `key: value` per line.
fn service_limits(service: &str, fields: &str) -> String {
    fields
        .lines()
        .fold(format!("services:\n  {service}:\n    limits:\n"), |mut block, line| {
            let _write_result = writeln!(block, "      {line}");
            block
        })
}

#[test]
fn limits_default_to_unset_and_are_left_out_when_unset() {
    let cluster: ClusterInput = serde_yaml::from_str(&scenario("", "")).unwrap();
    assert!(cluster.services.is_unset());
    assert!(cluster.services.hub.is_unset());
    assert!(cluster.services.collector.limits.is_unset());
    let written = serde_yaml::to_string(&cluster).unwrap();
    assert!(!written.contains("limits") && !written.contains("services"));
    assert!(ClusterInput::default().services.is_unset());
}

#[test]
fn the_services_take_only_the_settings_they_know() {
    let unknown_service = scenario("", "services:\n  router:\n    limits:\n      pids: 10\n");
    let _rejected = serde_yaml::from_str::<ClusterInput>(&unknown_service).unwrap_err();
    let unknown_setting = scenario("", "services:\n  hub:\n    replicas: 2\n");
    let _rejected = serde_yaml::from_str::<ClusterInput>(&unknown_setting).unwrap_err();
}

#[test]
fn every_limit_round_trips_through_yaml() {
    let input = scenario(&twin_limits(EVERY_LIMIT), &service_limits("hub", "cpus: 2"));
    let cluster: ClusterInput = serde_yaml::from_str(&input).unwrap();
    let written = serde_yaml::to_string(&cluster).unwrap();
    let reread: ClusterInput = serde_yaml::from_str(&written).unwrap();
    assert_eq!(serde_yaml::to_string(&reread).unwrap(), written);
    assert!(!reread.services.hub.is_unset());
    assert!(reread.services.collector.is_unset());

    let twin = &reread.agents[0].limits;
    assert_eq!(twin.memory.unwrap().as_u64(), 1536 << 20_u32);
    assert_eq!(twin.ulimits["core"], Ulimit::Both(0));
    assert_eq!(twin.ulimits["nofile"], Ulimit::Pair { soft: 1024, hard: 4096 });
    assert_eq!(Ulimit::Both(7).soft_hard(), (7, 7));
}

#[test]
fn cpus_are_a_positive_decimal_of_at_most_three_places() {
    for (raw, millicores) in [
        ("2", 2000),
        ("0.5", 500),
        ("\"1.25\"", 1250),
        ("0.001", 1),
        ("\" 3 \"", 3000),
    ] {
        assert_eq!(
            limits(&format!("cpus: {raw}")).unwrap().cpus,
            Some(Cpus::from_millicores(millicores))
        );
    }
    let rejected = [
        "0",
        "0.0",
        "1.2345",
        "-1",
        "\"abc\"",
        "\".5\"",
        "\"1e3\"",
        "5000000",
        "\"4294967296\"",
    ];
    for raw in rejected {
        let _rejected = limits(&format!("cpus: {raw}")).unwrap_err();
    }
    assert_eq!(Cpus::from_millicores(1250).to_string(), "1.25");
    assert_eq!(Cpus::from_millicores(500).to_string(), "0.5");
    assert_eq!(Cpus::from_millicores(3000).to_string(), "3");
    assert_eq!(Cpus::from_millicores(7).millicores(), 7);
}

#[test]
fn a_cpu_reservation_weighs_what_the_kubelet_gives_the_same_request() {
    assert_eq!(Cpus::from_millicores(100).shares(), 102);
    assert_eq!(Cpus::from_millicores(1000).shares(), 1024);
    assert_eq!(Cpus::from_millicores(1).shares(), 2);
}

#[test]
fn fields_neither_format_can_hold_a_process_to_are_rejected_by_name() {
    for field in [
        "cpu_count: 2",
        "mem_swappiness: 10",
        "oom_kill_disable: true",
        "oom_score_adj: -500",
        "cpu_rt_runtime: 950000",
    ] {
        let _rejected = limits(field).unwrap_err();
    }
}

#[test]
fn limits_docker_would_refuse_fail_generation_naming_the_process() {
    for (twin, reason) in [
        ("cpus: 0.5\ncpu_reservation: 1", "cpu_reservation 1 exceeds cpus 0.5"),
        ("memory: \"1 MiB\"\nmemory_reservation: \"2 MiB\"", "memory_reservation"),
        ("memory: \"2 MiB\"\nmemswap_limit: \"1 MiB\"", "memswap_limit"),
        ("memswap_limit: \"1 GiB\"", "is below memory 2.0 GiB"),
        ("cpus: 1\ncpu_quota: 50000", "cpus and cpu_quota set the same control"),
        ("cpus: 1\ncpu_period: 50000", "cpus and cpu_period set the same control"),
        ("cpu_reservation: 1\ncpu_shares: 512", "cpu_reservation and cpu_shares"),
        ("cpu_period: 50000", "cpu_period needs a cpu_quota"),
        ("cpu_quota: 999", "cpu_quota must be from 1000"),
        (
            "cpu_quota: 5000\ncpu_period: 999",
            "cpu_period must be from 1000 to 1000000",
        ),
        ("cpu_shares: 1", "cpu_shares must be from 2 to 262144"),
        ("blkio_weight: 5", "blkio_weight must be from 10 to 1000"),
        ("blkio_weight: 1001", "blkio_weight must be from 10 to 1000"),
        ("cpuset: \"a\"", "cpuset \"a\""),
        ("cpuset: \"\"", "cpuset \"\""),
        ("cpuset: \"-\"", "cpuset \"-\""),
        ("cpuset: \"0-\"", "cpuset \"0-\""),
        ("cpuset: \"0,,1\"", "cpuset \"0,,1\""),
        ("cpuset: \"3-1\"", "cpuset \"3-1\""),
        ("cpuset: \"0-1-2\"", "cpuset \"0-1-2\""),
        ("cpuset: \"+1\"", "cpuset \"+1\""),
        ("cpuset: \"99999999999\"", "cpuset \"99999999999\""),
        ("ulimits:\n  NOFILE: 1", "ulimit name \"NOFILE\""),
        ("ulimits:\n  \"\": 1", "ulimit name \"\""),
        (
            "ulimits:\n  nofile: {soft: 2, hard: 1}",
            "soft limit 2 above its hard limit 1",
        ),
    ] {
        let error = error_for(&twin_limits(twin), "");
        assert!(error.starts_with("Limits for \"math1-twin\" are invalid: "));
        assert!(error.contains(reason));
    }
    let bad = "cpu_shares: 1";
    assert!(error_for("", &service_limits("hub", bad)).starts_with("Limits for \"ws-server\""));
    assert!(error_for("", &service_limits("collector", bad)).starts_with("Limits for \"openobserve\""));
}

#[test]
fn a_cpuset_takes_a_list_of_cpus_and_ascending_ranges() {
    let (_root, dir) = generated(&scenario(&twin_limits("cpuset: \"0,2-3,5-5\""), ""));
    assert_eq!(compose_service(&dir, "math1-twin")["cpuset"], "0,2-3,5-5");
}

#[test]
fn a_scenario_cannot_take_the_container_runtime_plugins_namespace() {
    let test_root = tempdir().unwrap();
    let input_file = test_root.path().join("cluster.yaml");
    fs::write(&input_file, "cluster_name: \"nri-plugins\"\n").unwrap();
    let error = generate_deployment(&input_file, &test_root.path().join("output"), None).unwrap_err();
    assert!(error.to_string().contains("the container runtime plugins run in"));
}

#[test]
fn an_agent_without_a_runner_cannot_set_limits() {
    let input = concat!(
        "cluster_name: \"browser\"\n",
        "agents:\n",
        "  - name: \"in-browser\"\n",
        "    limits:\n",
        "      pids: 10\n",
        "    resources:\n",
        "      - type: \"math1\"\n",
    );
    let test_root = tempdir().unwrap();
    let input_file = test_root.path().join("cluster.yaml");
    fs::write(&input_file, input).unwrap();
    let error = generate_deployment(&input_file, &test_root.path().join("output"), None).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("\"in-browser\" sets limits but names no runner")
    );
}

#[test]
fn every_field_changes_what_both_container_formats_render() {
    let parity = [
        ("blkio_weight", "blkio_weight: 300"),
        ("cpu_period", "cpu_quota: 50000\ncpu_period: 50000"),
        ("cpu_quota", "cpu_quota: 50000"),
        ("cpu_reservation", "cpu_reservation: 0.5"),
        ("cpu_shares", "cpu_shares: 512"),
        ("cpus", "cpus: 1.5"),
        ("cpuset", "cpuset: \"0\""),
        ("memory", "memory: \"1 GiB\""),
        ("memory_reservation", "memory_reservation: \"256 MiB\""),
        ("memswap_limit", "memswap_limit: \"3 GiB\""),
        ("pids", "pids: 100"),
        ("shm_size", "shm_size: \"128 MiB\""),
        ("ulimits", "ulimits:\n  nofile: 4096"),
    ];
    let mut covered: Vec<String> = parity.iter().map(|(field, _)| (*field).to_owned()).collect();
    let mut schema = schema_fields();
    covered.sort();
    schema.sort();
    assert_eq!(covered, schema);

    let (_root, baseline) = generated(&scenario("", ""));
    let compose_baseline = compose_service(&baseline, "math1-twin");
    let k3s_baseline = deployment(&baseline, "math1-twin");
    for (_, fields) in parity {
        let (_root, dir) = generated(&scenario(&twin_limits(fields), ""));
        assert_ne!(compose_service(&dir, "math1-twin"), compose_baseline);
        assert_ne!(deployment(&dir, "math1-twin"), k3s_baseline);
    }
}

#[test]
fn compose_renders_every_limit() {
    let (_root, dir) = generated(&scenario(&twin_limits(EVERY_LIMIT), ""));
    let twin = compose_service(&dir, "math1-twin");
    let resources = &twin["deploy"]["resources"];
    for (value, expected) in [
        (&resources["limits"]["memory"], "1536m"),
        (&resources["reservations"]["memory"], "256m"),
        (&twin["cpuset"], "0-3"),
        (&twin["memswap_limit"], "2g"),
        (&twin["shm_size"], "128m"),
    ] {
        assert_eq!(value.as_str(), Some(expected));
    }
    for (value, expected) in [
        (&resources["limits"]["pids"], 256_i64),
        (&twin["blkio_config"]["weight"], 300_i64),
        (&twin["cpu_period"], 50_000_i64),
        (&twin["cpu_quota"], 75_000_i64),
        (&twin["cpu_shares"], 512_i64),
        (&twin["ulimits"]["core"]["soft"], 0_i64),
        (&twin["ulimits"]["core"]["hard"], 0_i64),
        (&twin["ulimits"]["nofile"]["soft"], 1_024_i64),
        (&twin["ulimits"]["nofile"]["hard"], 4_096_i64),
    ] {
        assert_eq!(value.as_i64(), Some(expected));
    }
    // A quota cap is the alternative to a CPU count, which Docker refuses alongside it.
    assert!(resources["limits"]["cpus"].is_null());
}

#[test]
fn compose_runs_every_process_under_the_defaults_k3s_has() {
    let (_root, dir) = generated(&scenario("", ""));
    for service in ["math1-trigger", "math1-twin", "openobserve", "ws-server"] {
        let compose = compose_service(&dir, service);
        assert_eq!(compose["deploy"]["resources"]["limits"]["cpus"].as_str(), Some("2"));
        assert_eq!(compose["deploy"]["resources"]["limits"]["memory"].as_str(), Some("2g"));
        // 102 shares is what the kubelet derives from the default 100m request.
        assert_eq!(compose["cpu_shares"].as_i64(), Some(102_i64));
        assert!(compose["deploy"]["resources"]["reservations"].is_null());
    }
    let hub_build = compose_service(&dir, "hub-image-build");
    assert_eq!(hub_build["deploy"], compose_service(&dir, "ws-server")["deploy"]);
}

#[test]
fn compose_writes_a_size_no_unit_divides_in_bytes() {
    let (_root, dir) = generated(&scenario(&twin_limits("cpu_reservation: 0.5\nshm_size: 1000"), ""));
    let twin = compose_service(&dir, "math1-twin");
    assert_eq!(twin["shm_size"].as_str(), Some("1000b"));
    assert_eq!(twin["cpu_shares"].as_i64(), Some(512_i64));
}

#[test]
fn k3s_applies_every_limit() {
    let (_root, dir) = generated(&scenario(&twin_limits(EVERY_LIMIT), ""));
    // The quota cap, 75000 per 50000, is 1.5 CPUs.
    assert_eq!(
        resources(&dir, "math1-twin"),
        quantities([["1500m", "1536Mi"], ["100m", "256Mi"]])
    );
    let expected = BTreeMap::from([
        ("cpu.max.memory-qos.nri.io/math1-twin", "75000 50000"),
        ("cpu.weight.memory-qos.nri.io/math1-twin", "59"),
        ("cpuset.cpus.memory-qos.nri.io/math1-twin", "0-3"),
        ("io.bfq.weight.memory-qos.nri.io/math1-twin", "default 300"),
        ("memory.low.memory-qos.nri.io/math1-twin", "268435456"),
        ("memory.swap.max.memory-qos.nri.io/math1-twin", "536870912"),
        ("pids.max.memory-qos.nri.io/math1-twin", "256"),
        (
            "required-plugins.noderesource.dev/container.math1-twin",
            "[\"nri-memory-qos\",\"ulimit-adjuster\"]",
        ),
        (
            "ulimits.noderesource.dev/container.math1-twin",
            concat!(
                "- type: RLIMIT_CORE\n  soft: 0\n  hard: 0\n",
                "- type: RLIMIT_NOFILE\n  soft: 1024\n  hard: 4096\n",
            ),
        ),
    ]);
    let actual = annotations(&dir, "math1-twin");
    let actual: BTreeMap<&str, &str> = actual
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    assert_eq!(actual, expected);

    let twin = pod(&dir, "math1-twin");
    let shm = twin
        .volumes
        .unwrap()
        .into_iter()
        .find(|volume| volume.name == "shm")
        .unwrap();
    let empty_dir = shm.empty_dir.unwrap();
    assert_eq!(empty_dir.medium.as_deref(), Some("Memory"));
    assert_eq!(empty_dir.size_limit.unwrap().0, "128Mi");
    let mounts = twin.containers[0].volume_mounts.clone().unwrap();
    assert!(
        mounts
            .iter()
            .any(|mount| mount.name == "shm" && mount.mount_path == "/dev/shm")
    );

    // The trigger sets nothing, so it names no plugin and could start on a node without them.
    assert!(annotations(&dir, "math1-trigger").is_empty());
    assert_eq!(
        resources(&dir, "math1-trigger"),
        quantities([["2", "2Gi"], ["100m", "128Mi"]])
    );
}

#[test]
fn k3s_runs_the_plugins_and_readies_the_nodes_only_when_a_process_needs_them() {
    let (_root, dir) = generated(&scenario(&twin_limits(EVERY_LIMIT), ""));
    let plugins = documents::<DaemonSet>(&dir, "DaemonSet");
    let images: Vec<String> = plugins
        .iter()
        .map(|plugin| {
            plugin.spec.clone().unwrap().template.spec.unwrap().containers[0]
                .image
                .clone()
                .unwrap()
        })
        .collect();
    assert_eq!(
        images,
        [
            "ghcr.io/containers/nri-plugins/nri-memory-qos:v0.14.0",
            "ghcr.io/containerd/nri/plugins/ulimit-adjuster:v0.10.0",
        ]
    );
    assert!(
        plugins
            .iter()
            .all(|plugin| plugin.metadata.namespace.as_deref() == Some("et-nri-plugins"))
    );
    let config = documents::<ConfigMap>(&dir, "ConfigMap")
        .into_iter()
        .find(|map| map.metadata.name.as_deref() == Some("nri-memory-qos-config"))
        .unwrap();
    let config = &config.data.unwrap()["config.yaml"];
    for file in ["cpu.max", "cpu.weight", "io.bfq.weight", "memory.swap.max", "pids.max"] {
        assert!(config.contains(&format!("- {file}\n")));
    }
    assert!(read(&dir, "containerd-config-v3.toml.tmpl").contains("default_validator]\n  enable = true"));
    let readme = read(&dir, "README.md");
    assert!(readme.contains("### Prepare The Nodes"));
    assert!(readme.contains("`nri-memory-qos` and `ulimit-adjuster`"));
    assert!(readme.find("### Prepare The Nodes") < readme.find("### Load The Credential"));

    // Only ulimits: only the one plugin, and no memory-qos configuration.
    let (_root, dir) = generated(&scenario(&twin_limits("ulimits:\n  nofile: 4096"), ""));
    assert_eq!(documents::<DaemonSet>(&dir, "DaemonSet").len(), 1);
    assert!(read(&dir, "k3s.yaml").contains("ulimit-adjuster"));
    assert!(!read(&dir, "k3s.yaml").contains("nri-memory-qos"));

    // Limits the Pod API carries itself need no plugin, so the manifests stay as they were.
    let (_root, dir) = generated(&scenario(
        &twin_limits("cpus: 1\nmemory: \"1 GiB\"\nshm_size: \"64 MiB\""),
        "",
    ));
    assert_eq!(documents::<DaemonSet>(&dir, "DaemonSet"), Vec::<DaemonSet>::new());
    assert!(!dir.join("containerd-config-v3.toml.tmpl").exists());
    assert!(!read(&dir, "README.md").contains("Prepare The Nodes"));
}

#[test]
fn the_plugins_run_under_the_defaults_a_process_setting_no_limits_resolves_to() {
    let (_root, dir) = generated(&scenario(&twin_limits(EVERY_LIMIT), ""));
    let trigger = pod(&dir, "math1-trigger").containers[0].resources.clone();
    for plugin in documents::<DaemonSet>(&dir, "DaemonSet") {
        let container = &plugin.spec.unwrap().template.spec.unwrap().containers[0];
        assert_eq!(container.resources, trigger);
    }
}

#[test]
fn k3s_weights_cpu_shares_by_runc_s_own_conversion() {
    // The weights runc writes for Docker's `--cpu-shares`, read back from a container on a cgroup v2 host.
    for (shares, weight) in [
        (2_u64, "1"),
        (102, "17"),
        (512, "59"),
        (1024, "100"),
        (1 << 18_u32, "10000"),
    ] {
        let (_root, dir) = generated(&scenario(&twin_limits(&format!("cpu_shares: {shares}")), ""));
        assert_eq!(
            annotations(&dir, "math1-twin")["cpu.weight.memory-qos.nri.io/math1-twin"],
            weight
        );
    }
}

#[test]
fn a_default_never_crosses_a_limit_the_scenario_set_in_either_format() {
    for (twin, expected, cpus, memory, shares) in [
        (
            "cpus: 0.05\nmemory: \"64 MiB\"",
            [["50m", "64Mi"], ["50m", "64Mi"]],
            "0.05",
            "64m",
            51_i64,
        ),
        (
            "cpu_reservation: 3\nmemory_reservation: \"4 GiB\"",
            [["3", "4Gi"], ["3", "4Gi"]],
            "3",
            "4g",
            3_072_i64,
        ),
        (
            "cpu_reservation: 0.5\nmemory_reservation: 1000",
            [["2", "2Gi"], ["500m", "1000"]],
            "2",
            "2g",
            512_i64,
        ),
    ] {
        let (_root, dir) = generated(&scenario(&twin_limits(twin), ""));
        assert_eq!(resources(&dir, "math1-twin"), quantities(expected));
        let compose = compose_service(&dir, "math1-twin");
        let limit = &compose["deploy"]["resources"]["limits"];
        assert_eq!(limit["cpus"].as_str(), Some(cpus));
        assert_eq!(limit["memory"].as_str(), Some(memory));
        assert_eq!(compose["cpu_shares"].as_i64(), Some(shares));
    }
}

#[test]
fn the_readme_names_the_limits_only_the_host_formats_do_not_apply() {
    let (_root, dir) = generated(&scenario(&twin_limits(EVERY_LIMIT), &service_limits("hub", "cpus: 2")));
    let readme = read(&dir, "README.md");
    for heading in ["Mise", "dekit"] {
        let section = readme
            .split(&format!("### Limits Not Applied With {heading}"))
            .nth(1)
            .unwrap();
        let listed = section.split("\n#").next().unwrap();
        assert!(listed.contains("- `ws-server`\n") && listed.contains("- `math1-twin`\n"));
        assert!(!listed.contains("math1-trigger"));
    }
    assert!(!readme.contains("Limits Not Applied With k3s"));
    assert!(!readme.contains("Limits Not Applied With Docker Compose"));

    let (_root, dir) = generated(&scenario("", ""));
    assert!(!read(&dir, "README.md").contains("Limits Not Applied"));
}
