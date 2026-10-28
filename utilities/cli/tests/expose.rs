//! Covers `expose:`: which ports a scenario publishes outside its cluster, and the checks that keep them apart.
//!
//! k3s is the one format with a boundary to publish across, so most cases read its manifests; the host-network
//! formats are covered by their README and by the collision checks, which hold in every format.
#![cfg(test)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use et_cli::{ClusterInput, OutputType, generate_deployment};
use fs_err as fs;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{Service, ServicePort};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use k8s_openapi::{Metadata, Resource};
use serde::Deserialize as _;
use serde::de::DeserializeOwned;
use tempfile::tempdir;

/// A browser agent, so the hub serves a page, and a pyo3 runner whose agent `{expose}` is spliced into.
const BROWSER_AND_RUNNER: &str = r#"cluster_name: "exposed"
agents:
  - name: "har"
    resources:
      - type: "har1"
  - name: "twin"
    runner: "pyo3"
    resources:
      - type: "pyo3-math1"
"#;

/// A headless scenario: one pyo3 runner and nothing a browser loads.
const HEADLESS: &str = r#"cluster_name: "headless"
agents:
  - name: "twin"
    runner: "pyo3"
    resources:
      - type: "pyo3-math1"
"#;

/// The twin's ports, one of them UDP.
const TWIN_EXPOSE: &str = r#"    expose:
      - name: "http"
        port: 8000
      - name: "metrics"
        port: 9000
        protocol: "udp"
"#;

/// The hub's two ports.
const HUB_EXPOSE: &str = "services:\n  hub:\n    expose: [\"ws\", \"wss\"]\n";

/// Two machines each running a hub, the twin on `edge`.
const MACHINES: &str = r#"machines:
  - name: "server"
    address: "10.0.0.1"
  - name: "edge"
    address: "10.0.0.2"
    agents: ["twin"]
services:
  hub:
    machines: ["server", "edge"]
    expose: ["ws"]
"#;

/// A web runner exposing the twin's HTTP port, appended after the twin.
const OTHER: &str = r#"  - name: "other"
    runner: "web"
    expose:
      - name: "http"
        port: 8000
    resources:
      - type: "math1"
"#;

/// The files generating `input` as `output_type` writes, read back by name, with the directory they were written to.
struct Generated {
    dir: tempfile::TempDir,
}

impl Generated {
    fn new(input: &str, output_type: OutputType) -> Self {
        let dir = tempdir().unwrap();
        let input_file = dir.path().join("cluster.yaml");
        fs::write(&input_file, input).unwrap();
        let _summary = generate_deployment(&input_file, &dir.path().join("out"), Some(output_type)).unwrap();
        Self { dir }
    }

    fn read(&self, file: &str) -> String {
        fs::read_to_string(self.out().join(file)).unwrap()
    }

    fn out(&self) -> PathBuf {
        self.dir.path().join("out")
    }

    /// Every k3s object of type `T`, by name.
    fn objects<T>(&self) -> BTreeMap<String, T>
    where
        T: DeserializeOwned + Metadata<Ty = ObjectMeta> + Resource,
    {
        let manifest = self.read("k3s.yaml");
        let mut objects = BTreeMap::new();
        for document in serde_yaml::Deserializer::from_str(&manifest) {
            let value = serde_yaml::Value::deserialize(document).unwrap();
            if value["kind"] != T::KIND {
                continue;
            }
            let object: T = serde_yaml::from_value(value).unwrap();
            let _previous = objects.insert(object.metadata().name.clone().unwrap(), object);
        }
        objects
    }

    fn services(&self) -> BTreeMap<String, Service> {
        self.objects()
    }
}

/// The twin's agent with `expose` spliced in after its runner.
fn with_twin_expose(base: &str) -> String {
    base.replace(
        "    runner: \"pyo3\"\n",
        &format!("    runner: \"pyo3\"\n{TWIN_EXPOSE}"),
    )
}

/// The error generating `input` produces.
fn error_for(input: &str) -> String {
    let dir = tempdir().unwrap();
    let input_file = dir.path().join("cluster.yaml");
    fs::write(&input_file, input).unwrap();
    generate_deployment(&input_file, &dir.path().join("out"), Some(OutputType::K3s))
        .unwrap_err()
        .to_string()
}

/// The `(name, port, protocol)` of each of a `Service`'s ports.
fn service_ports(service: &Service) -> Vec<(String, i32, String)> {
    let ports: &[ServicePort] = service.spec.as_ref().unwrap().ports.as_deref().unwrap();
    ports
        .iter()
        .map(|port| (port.name.clone().unwrap(), port.port, port.protocol.clone().unwrap()))
        .collect()
}

/// A `Service`'s type, its traffic policy and its load-balancer pool label, each empty when unset.
fn exposure(service: &Service) -> (String, String, String) {
    let spec = service.spec.as_ref().unwrap();
    let pool = service
        .metadata
        .labels
        .as_ref()
        .and_then(|labels| labels.get("svccontroller.k3s.cattle.io/lbpool"))
        .cloned()
        .unwrap_or_default();
    (
        spec.type_.clone().unwrap_or_default(),
        spec.external_traffic_policy.clone().unwrap_or_default(),
        pool,
    )
}

fn selector(service: &Service) -> String {
    service.spec.as_ref().unwrap().selector.as_ref().unwrap()["app"].clone()
}

fn tcp(name: &str, port: i32) -> (String, i32, String) {
    (name.to_owned(), port, "TCP".to_owned())
}

#[test]
fn expose_defaults_to_none_and_is_left_out_when_unset() {
    let cluster: ClusterInput = serde_yaml::from_str(BROWSER_AND_RUNNER).unwrap();
    let written = serde_yaml::to_string(&cluster).unwrap();
    assert!(!written.contains("expose:"));

    let cluster: ClusterInput = serde_yaml::from_str(&with_twin_expose(BROWSER_AND_RUNNER)).unwrap();
    let written = serde_yaml::to_string(&cluster).unwrap();
    assert!(written.contains("expose:"));
    assert_eq!(written.matches("protocol:").count(), 1);
}

#[test]
fn nothing_is_published_unless_exposed() {
    let generated = Generated::new(BROWSER_AND_RUNNER, OutputType::K3s);
    let services = generated.services();
    assert_eq!(services.keys().collect::<Vec<_>>(), ["openobserve", "ws-server"]);
    assert_eq!(exposure(&services["ws-server"]), Default::default());
    let readme = generated.read("README.md");
    assert!(readme.contains("### Open From Outside The Cluster"));
    assert!(readme.contains("no browser can open its page"));
}

#[test]
fn a_headless_scenario_says_nothing_about_outside_access() {
    let readme = Generated::new(HEADLESS, OutputType::K3s).read("README.md");
    assert!(!readme.contains("### Open From Outside The Cluster"));
    assert!(!readme.contains("lbpool"));
}

#[test]
fn an_exposed_hub_gets_a_load_balancer_beside_its_cluster_service() {
    let generated = Generated::new(&format!("{BROWSER_AND_RUNNER}{HUB_EXPOSE}"), OutputType::K3s);
    let services = generated.services();
    assert_eq!(exposure(&services["ws-server"]), Default::default());
    let external = &services["ws-server-ext"];
    assert_eq!(
        exposure(external),
        ("LoadBalancer".to_owned(), "Local".to_owned(), String::default())
    );
    assert_eq!(selector(external), "ws-server");
    assert_eq!(service_ports(external), [tcp("ws", 8080), tcp("wss", 8443)]);
    let readme = generated.read("README.md");
    assert!(readme.contains("- `ws-server` `ws`: <http://localhost:8080/>"));
    assert!(readme.contains("- `ws-server` `wss`: <https://localhost:8443/>"));
    assert!(!readme.contains("no browser can open its page"));
}

#[test]
fn a_hub_publishes_only_the_ports_listed() {
    let input = format!("{BROWSER_AND_RUNNER}services:\n  hub:\n    expose: [\"wss\"]\n");
    let services = Generated::new(&input, OutputType::K3s).services();
    assert_eq!(service_ports(&services["ws-server-ext"]), [tcp("wss", 8443)]);
}

#[test]
fn an_exposed_runner_gets_a_load_balancer_and_declares_its_ports() {
    let generated = Generated::new(&with_twin_expose(BROWSER_AND_RUNNER), OutputType::K3s);
    let twin = &generated.services()["twin"];
    assert_eq!(
        exposure(twin),
        ("LoadBalancer".to_owned(), "Local".to_owned(), String::default())
    );
    assert_eq!(selector(twin), "twin");
    assert_eq!(
        service_ports(twin),
        [tcp("http", 8000), ("metrics".to_owned(), 9000_i32, "UDP".to_owned())]
    );

    let deployments: BTreeMap<String, Deployment> = generated.objects();
    let pod = deployments["twin"].spec.clone().unwrap().template.spec.unwrap();
    let ports: Vec<(String, i32, String)> = pod.containers[0]
        .ports
        .clone()
        .unwrap()
        .into_iter()
        .map(|port| (port.name.unwrap(), port.container_port, port.protocol.unwrap()))
        .collect();
    assert_eq!(ports, service_ports(twin));

    let readme = generated.read("README.md");
    assert!(readme.contains("- `twin` `http`: <http://localhost:8000/>"));
    assert!(readme.contains("- `twin` `metrics`: `localhost:9000/udp`"));
}

#[test]
fn each_hub_is_published_on_its_own_machine() {
    let generated = Generated::new(&format!("{}{MACHINES}", with_twin_expose(HEADLESS)), OutputType::K3s);
    let services = generated.services();
    assert_eq!(exposure(&services["ws-server-ext"]).2, "server");
    assert_eq!(exposure(&services["ws-server-edge-ext"]).2, "edge");
    assert_eq!(selector(&services["ws-server-edge-ext"]), "ws-server-edge");
    assert_eq!(exposure(&services["twin"]).2, "edge");

    let readme = generated.read("README.md");
    assert!(readme.contains("- `ws-server` `ws`: <http://10.0.0.1:8080/>"));
    assert!(readme.contains("- `ws-server-edge` `ws`: <http://10.0.0.2:8080/>"));
    assert!(readme.contains("- `twin` `http`: <http://10.0.0.2:8000/>"));
    assert!(readme.contains("--node-label svccontroller.k3s.cattle.io/lbpool=server"));
    assert!(readme.contains("--node-label svccontroller.k3s.cattle.io/lbpool=edge"));
}

#[test]
fn machines_join_no_pool_when_nothing_is_exposed() {
    let input = format!("{HEADLESS}{}", MACHINES.replace("    expose: [\"ws\"]\n", ""));
    let readme = Generated::new(&input, OutputType::K3s).read("README.md");
    assert!(readme.contains("--node-label"));
    assert!(!readme.contains("lbpool"));
}

#[test]
fn the_compose_readme_lists_runner_ports_alone() {
    let input = format!("{}{HUB_EXPOSE}", with_twin_expose(BROWSER_AND_RUNNER));
    let readme = Generated::new(&input, OutputType::DockerCompose).read("README.md");
    assert!(readme.contains("The runners listen on their host's network too, at:"));
    assert!(readme.contains("- `twin` `http`: <http://localhost:8000/>"));
    assert!(!readme.contains("- `ws-server`"));

    let readme = Generated::new(BROWSER_AND_RUNNER, OutputType::DockerCompose).read("README.md");
    assert!(!readme.contains("The runners listen"));
}

#[test]
fn only_a_runner_that_can_listen_exposes_ports() {
    let browser = BROWSER_AND_RUNNER.replace(
        "  - name: \"har\"\n",
        "  - name: \"har\"\n    expose:\n      - name: \"http\"\n        port: 8000\n",
    );
    assert!(error_for(&browser).contains("Agent \"har\" exposes ports but names no runner"));

    let wasi = with_twin_expose(BROWSER_AND_RUNNER)
        .replace("runner: \"pyo3\"", "runner: \"wasi\"")
        .replace("pyo3-math1", "wasi-math1");
    assert!(error_for(&wasi).contains("the wasi runner grants its component no network access"));

    let two = with_twin_expose(BROWSER_AND_RUNNER).replace(
        "      - type: \"pyo3-math1\"\n",
        "      - type: \"pyo3-math1\"\n      - type: \"math1\"\n",
    );
    assert!(error_for(&two).contains("has more than one resource"));

    let scheduled = with_twin_expose(BROWSER_AND_RUNNER).replace(
        "    runner: \"pyo3\"\n",
        "    runner: \"pyo3\"\n    schedule:\n      cron: \"* * * * *\"\n",
    );
    assert!(error_for(&scheduled).contains("sets a schedule, and a runner started per tick is not a server"));
}

#[test]
fn hub_ports_are_named_from_its_own() {
    let unknown = format!("{BROWSER_AND_RUNNER}services:\n  hub:\n    expose: [\"http\"]\n");
    assert!(error_for(&unknown).contains("services.hub.expose lists \"http\"; the hub's ports are ws, wss"));

    let twice = format!("{BROWSER_AND_RUNNER}services:\n  hub:\n    expose: [\"ws\", \"ws\"]\n");
    assert!(error_for(&twice).contains("services.hub exposes a port named \"ws\" more than once"));
}

#[test]
fn an_external_hub_service_name_must_fit_a_kubernetes_object_name() {
    // Long enough for the hub's own name to fit, and too long once the external service's suffix is added.
    let long = "e".repeat(52);
    let input = format!("{HEADLESS}{}", MACHINES.replace("edge", &long));
    let error = error_for(&input);
    assert!(error.contains(&format!("would run hub \"ws-server-{long}-ext\"")));
    assert!(error.contains("it is longer than 63 characters"));
}

#[test]
fn agent_ports_are_well_formed() {
    let renamed = |from: &str, into: &str| with_twin_expose(BROWSER_AND_RUNNER).replace(from, into);
    let upper = renamed("name: \"http\"", "name: \"HTTP\"");
    assert!(error_for(&upper).contains("exposes a port named \"HTTP\", which is not a Kubernetes port name"));

    let long = renamed("name: \"http\"", "name: \"a-very-long-port-name\"");
    assert!(error_for(&long).contains("it is longer than 15 characters"));

    let numeric = renamed("name: \"http\"", "name: \"8000\"");
    let error = error_for(&numeric);
    assert!(error.contains("exposes a port named \"8000\", which is not a Kubernetes port name: it has no letter"));

    let hyphens = renamed("name: \"http\"", "name: \"h--p\"");
    assert!(error_for(&hyphens).contains("it contains consecutive '-'"));

    let zero = renamed("port: 8000", "port: 0");
    assert!(error_for(&zero).contains("Agent \"twin\" exposes port \"http\" as 0"));

    let twice = renamed("name: \"metrics\"", "name: \"http\"");
    assert!(error_for(&twice).contains("Agent \"twin\" exposes a port named \"http\" more than once"));
}

#[test]
fn two_processes_on_one_machine_cannot_share_a_port() {
    let on_hub = with_twin_expose(BROWSER_AND_RUNNER).replace("port: 8000", "port: 8080");
    let expected = "Agent \"twin\" exposes 8080/tcp on this host, where `ws-server` already uses it";
    assert!(error_for(&on_hub).contains(expected));

    let pair = format!("{}{OTHER}", with_twin_expose(HEADLESS));
    assert!(error_for(&pair).contains("Agent \"other\" exposes 8000/tcp on this host, where Agent \"twin\""));

    // The same port over UDP is a different port.
    let udp = pair.replace(
        "port: 8000\n    resources",
        "port: 8000\n        protocol: \"udp\"\n    resources",
    );
    let _generated = Generated::new(&udp, OutputType::K3s);
}

#[test]
fn the_same_port_on_different_machines_is_two_ports() {
    let input = format!("{}{OTHER}{MACHINES}", with_twin_expose(HEADLESS));
    let services = Generated::new(&input, OutputType::K3s).services();
    assert_eq!(exposure(&services["other"]).2, "server");
    assert_eq!(exposure(&services["twin"]).2, "edge");
}
