//! Covers a scenario that runs a hub on more than one machine: how `services.hub.machines` is validated, and how each
//! format runs every hub, joins them through the NATS backplane and points them at the one object store.
//!
//! The fixture has three machines: the server, with the trigger; `edge`, which runs a hub of its own and the twin;
//! and `far`, which runs none and so reaches the server's hub across the network.
#![cfg(test)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use et_cli::{
    ClusterInput, generate_deployment, hub_service_ws_url, hub_service_ws_url_named, hub_ws_url, hub_ws_url_at,
    regenerate_verification,
};
use fs_err as fs;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::Container;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use serde::Deserialize as _;
use tempfile::{TempDir, tempdir};

/// The agents and machines every scenario here declares, with the `services:` table appended after them.
const CLUSTER: &str = r#"cluster_name: "hubs"
agents:
  - name: "math1-twin"
    runner: "web"
    resources:
      - type: "math1"
  - name: "math1-trigger"
    runner: "web"
    resources:
      - type: "math1-sender"
  - name: "wasi-twin"
    runner: "wasi"
    resources:
      - type: "wasi-math1"
machines:
  - name: "server"
    address: "10.0.0.1"
    agents: ["math1-trigger"]
  - name: "edge"
    address: "10.0.0.2"
    agents: ["math1-twin"]
  - name: "far"
    address: "10.0.0.3"
    agents: ["wasi-twin"]
"#;

/// Hubs on the server and on `edge`, with the shared services given limits of their own.
const SERVICES: &str = r#"services:
  hub:
    machines: ["server", "edge"]
  backplane:
    limits:
      memory: "256m"
  storage:
    limits:
      memory: "512m"
"#;

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

fn yaml(dir: &Path, file: &str) -> serde_yaml::Value {
    serde_yaml::from_str(&read(dir, file)).unwrap()
}

fn toml(dir: &Path) -> toml::Table {
    read(dir, "mise.toml").parse().unwrap()
}

/// The error generating `input` produces.
fn error_for(input: &str) -> String {
    let test_root = tempdir().unwrap();
    let input_file = test_root.path().join("cluster.yaml");
    fs::write(&input_file, input).unwrap();
    generate_deployment(&input_file, &test_root.path().join("output"), None)
        .unwrap_err()
        .to_string()
}

/// Any Kubernetes object, read only as far as its name.
#[derive(serde::Deserialize)]
struct Named {
    metadata: ObjectMeta,
}

/// Every k3s document of `kind`, by name.
fn k3s_documents(dir: &Path, kind: &str) -> BTreeMap<String, serde_yaml::Value> {
    let manifest = read(dir, "k3s.yaml");
    serde_yaml::Deserializer::from_str(&manifest)
        .map(|document| serde_yaml::Value::deserialize(document).unwrap())
        .filter(|document| document["kind"] == kind)
        .map(|document| {
            let named: Named = serde_yaml::from_value(document.clone()).unwrap();
            (named.metadata.name.unwrap(), document)
        })
        .collect()
}

/// The one container of each k3s `Deployment`, with the machine its pods are pinned to.
fn k3s_containers(dir: &Path) -> BTreeMap<String, (Container, String)> {
    k3s_documents(dir, "Deployment")
        .into_iter()
        .map(|(name, document)| {
            let deployment: Deployment = serde_yaml::from_value(document).unwrap();
            let pod = deployment.spec.unwrap().template.spec.unwrap();
            let machine = pod.node_selector.unwrap_or_default().into_values().collect::<String>();
            (name, (pod.containers[0].clone(), machine))
        })
        .collect()
}

/// A k3s container's literal environment, by name.
fn container_env(container: &Container) -> BTreeMap<String, String> {
    container
        .env
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|var| (var.name, var.value.unwrap_or_default()))
        .collect()
}

#[test]
fn hub_machines_default_to_none_and_are_left_out_when_unset() {
    let cluster: ClusterInput = serde_yaml::from_str(CLUSTER).unwrap();
    assert_eq!(cluster.services.hub.machines, Vec::<String>::new());
    assert!(cluster.services.hub.is_unset());
    assert!(cluster.services.backplane.is_unset());
    assert!(cluster.services.storage.is_unset());
    assert!(!serde_yaml::to_string(&cluster).unwrap().contains("services"));

    let cluster: ClusterInput = serde_yaml::from_str(&format!("{CLUSTER}{SERVICES}")).unwrap();
    assert_eq!(cluster.services.hub.machines, ["server", "edge"]);
    assert!(!cluster.services.hub.is_unset());
    let written = serde_yaml::to_string(&cluster).unwrap();
    assert!(written.contains("backplane:"));
    assert!(written.contains("storage:"));
}

#[test]
fn hub_machines_need_machines() {
    let input = "cluster_name: \"bare\"\nservices:\n  hub:\n    machines: [\"server\"]\n";
    assert!(error_for(input).contains("services.hub.machines lists \"server\", but the scenario has no machines"));
}

#[test]
fn hub_machines_must_be_listed_machines() {
    let error = error_for(&format!(
        "{CLUSTER}services:\n  hub:\n    machines: [\"server\", \"nowhere\"]\n"
    ));
    assert!(error.contains("services.hub.machines lists \"nowhere\", which is not one of the scenario's machines"));
}

#[test]
fn a_hub_machine_is_listed_once() {
    let error = error_for(&format!(
        "{CLUSTER}services:\n  hub:\n    machines: [\"server\", \"edge\", \"edge\"]\n"
    ));
    assert!(error.contains("services.hub.machines lists \"edge\" more than once"));
}

#[test]
fn hub_machines_include_the_server() {
    let error = error_for(&format!("{CLUSTER}services:\n  hub:\n    machines: [\"edge\"]\n"));
    assert!(error.contains("services.hub.machines leaves out the server \"server\""));
}

#[test]
fn a_shared_service_needs_several_hubs() {
    for service in ["backplane", "storage"] {
        let error = error_for(&format!(
            "{CLUSTER}services:\n  {service}:\n    limits:\n      memory: \"256m\"\n"
        ));
        assert!(error.contains(&format!("services.{service} is set, but only several hubs run it")));
    }
}

#[test]
fn a_runner_cannot_take_a_hub_or_shared_service_name() {
    for name in ["ws-server-edge", "nats", "rustfs"] {
        let input = format!("{CLUSTER}{SERVICES}").replace("\"wasi-twin\"", &format!("\"{name}\""));
        let expected = format!("would create runner \"{name}\", a name the generated deployment");
        assert!(error_for(&input).contains(&expected));
    }
}

#[test]
fn the_server_alone_is_one_hub_and_shares_nothing() {
    let (_root, dir) = generated(&format!("{CLUSTER}services:\n  hub:\n    machines: [\"server\"]\n"));
    let tasks = toml(&dir)["tasks"].as_table().unwrap().clone();
    assert!(tasks.contains_key("ws-server"));
    assert!(!tasks.contains_key("nats"));
    assert!(!tasks.contains_key("rustfs"));
    assert!(!read(&dir, "secrets.env").contains("AWS_SECRET_ACCESS_KEY"));
    assert!(k3s_documents(&dir, "PersistentVolumeClaim").contains_key("ws-server-storage"));
}

#[test]
fn the_credential_file_carries_the_storage_secret_under_both_names() {
    let (_root, dir) = generated(&format!("{CLUSTER}{SERVICES}"));
    let secrets = read(&dir, "secrets.env");
    let password = secrets
        .lines()
        .find_map(|line| line.strip_prefix("OTLP_AUTH_PASSWORD="))
        .unwrap();
    assert!(secrets.contains(&format!("\nAWS_SECRET_ACCESS_KEY={password}\n")));
    assert!(secrets.contains(&format!("\nRUSTFS_SECRET_KEY={password}\n")));
}

/// The mise tasks generated from the fixture with its hubs, by name.
fn mise_tasks() -> toml::Table {
    let (_root, dir) = generated(&format!("{CLUSTER}{SERVICES}"));
    toml(&dir)["tasks"].as_table().unwrap().clone()
}

/// The names a mise task depends on, in the order the file lists them.
fn depends(tasks: &toml::Table, task: &str) -> Vec<String> {
    tasks[task]["depends"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn mise_starts_each_hub_with_its_machine() {
    let tasks = mise_tasks();
    assert_eq!(
        depends(&tasks, "machine-server"),
        ["math1-trigger", "nats", "openobserve", "rustfs", "ws-server"]
    );
    assert_eq!(depends(&tasks, "machine-edge"), ["math1-twin", "ws-server-edge"]);
    assert_eq!(depends(&tasks, "machine-far"), ["wasi-twin"]);
    assert!(!depends(&tasks, "generated-scenario").contains(&"ws-server-edge".to_string()));
    assert_eq!(
        tasks["ws-server-edge"]["description"].as_str(),
        Some("Run the WebSocket server on edge")
    );
}

#[test]
fn mise_hubs_reach_the_shared_services_at_the_server() {
    let tasks = mise_tasks();
    let server_env = tasks["ws-server"]["env"].as_table().unwrap();
    assert_eq!(server_env["WS_BACKPLANE"].as_str(), Some("nats://localhost:4222"));
    assert_eq!(server_env["AWS_ENDPOINT"].as_str(), Some("http://localhost:9000"));
    assert_eq!(server_env["STORAGE_URL"].as_str(), Some("s3://et-storage"));
    let edge_env = tasks["ws-server-edge"]["env"].as_table().unwrap();
    assert_eq!(edge_env["WS_BACKPLANE"].as_str(), Some("nats://10.0.0.1:4222"));
    assert_eq!(edge_env["AWS_ENDPOINT"].as_str(), Some("http://10.0.0.1:9000"));
}

#[test]
fn mise_runners_join_their_own_machines_hub_or_the_servers() {
    let tasks = mise_tasks();
    let url = |task: &str| tasks[task]["env"]["WS_SERVER_URL"].as_str().unwrap().to_string();
    assert_eq!(url("math1-twin"), hub_ws_url());
    assert_eq!(url("wasi-twin"), hub_ws_url_at("10.0.0.1"));
}

#[test]
fn mise_runs_the_shared_services_as_containers() {
    let tasks = mise_tasks();
    let nats = tasks["nats"]["run"].as_str().unwrap();
    assert!(nats.contains("image=nats:2.15.1-alpine\n"));
    assert!(nats.contains("container=nats-hubs\n"));
    assert!(nats.contains("--jetstream --store_dir /data --http_port 8222"));
    let rustfs = tasks["rustfs"]["run"].as_str().unwrap();
    assert!(rustfs.contains("image=rustfs/rustfs:1.0.0-beta.12\n"));
    assert!(rustfs.contains("-e RUSTFS_ACCESS_KEY=et-storage -e RUSTFS_VOLUMES=/data -e RUSTFS_SECRET_KEY"));
    assert!(rustfs.contains("start='mkdir -p /data/et-storage && exec /entrypoint.sh rustfs'"));
}

#[test]
fn dekit_waits_on_the_shared_services_on_the_server_only() {
    let (_root, dir) = generated(&format!("{CLUSTER}{SERVICES}"));
    let dekit = yaml(&dir, "dekit.yaml");
    let tasks = &dekit["tasks"];
    assert_eq!(
        tasks["ws-server"]["deps"],
        serde_yaml::from_str::<serde_yaml::Value>("[openobserve, nats, rustfs]").unwrap()
    );
    assert!(tasks["ws-server-edge"].get("deps").is_none());
    assert_eq!(tasks["ws-server-edge"]["autorestart"], "on-failure");
    assert_eq!(tasks["ws-server-edge"]["tags"][0], "edge");
    assert_eq!(tasks["math1-twin"]["deps"][0], "ws-server-edge");
    assert!(tasks["wasi-twin"].get("deps").is_none());
    assert_eq!(
        tasks["nats"]["ready"]["http"],
        "http://localhost:8222/healthz?js-enabled-only=true"
    );
    assert_eq!(tasks["rustfs"]["stop"]["cmd"][2], "rustfs-hubs");
}

#[test]
fn process_compose_waits_on_the_shared_services_on_the_server_only() {
    let (_root, dir) = generated(&format!("{CLUSTER}{SERVICES}"));
    let process_compose = yaml(&dir, "process-compose.yaml");
    let processes = &process_compose["processes"];
    assert!(processes["ws-server"]["depends_on"].get("rustfs").is_some());
    assert!(processes["ws-server-edge"].get("depends_on").is_none());
    assert_eq!(processes["ws-server-edge"]["availability"]["restart"], "on_failure");
    assert_eq!(processes["ws-server-edge"]["namespace"], "edge");
    assert!(processes["math1-twin"]["depends_on"].get("ws-server-edge").is_some());
    assert_eq!(
        processes["rustfs"]["readiness_probe"]["http_get"]["port"].as_u64(),
        Some(9_000_u64)
    );
    assert_eq!(processes["nats"]["shutdown"]["command"], "docker stop nats-hubs");
}

/// The compose services generated from the fixture with its hubs, by name, and the volumes the file declares.
fn compose_file() -> serde_yaml::Value {
    let (_root, dir) = generated(&format!("{CLUSTER}{SERVICES}"));
    yaml(&dir, "compose.yaml")
}

#[test]
fn compose_runs_a_hub_per_machine_profile() {
    let compose = compose_file();
    let services = &compose["services"];
    assert_eq!(services["ws-server"]["profiles"][0], "server");
    assert_eq!(services["ws-server-edge"]["profiles"][0], "edge");
    assert_eq!(services["ws-server-edge"]["restart"], "on-failure");
    assert_eq!(
        services["ws-server-edge"]["depends_on"]["nats"]["required"].as_bool(),
        Some(false)
    );
    assert!(services["ws-server"]["depends_on"]["rustfs"].get("required").is_none());
    assert!(services["ws-server"].get("volumes").is_none());
    assert_eq!(
        services["hub-image-build"]["profiles"],
        serde_yaml::from_str::<serde_yaml::Value>("[server, edge]").unwrap()
    );
}

#[test]
fn compose_hubs_share_the_servers_services_and_runners_join_their_own_hub() {
    let compose = compose_file();
    let services = &compose["services"];
    let edge_env = &services["ws-server-edge"]["environment"];
    assert_eq!(edge_env["WS_BACKPLANE"], "nats://10.0.0.1:4222");
    assert_eq!(edge_env["AWS_ALLOW_HTTP"], "true");
    assert_eq!(edge_env["STORAGE_URL"], "s3://et-storage");
    assert_eq!(
        services["math1-twin"]["environment"]["WS_SERVER_URL"].as_str(),
        Some(hub_ws_url().as_str())
    );
    assert!(services["math1-twin"]["depends_on"].get("ws-server-edge").is_some());
    assert_eq!(
        services["wasi-twin"]["depends_on"]["ws-server"]["required"].as_bool(),
        Some(false)
    );
}

#[test]
fn compose_runs_the_shared_services_on_the_server_under_their_limits() {
    let compose = compose_file();
    let services = &compose["services"];
    assert_eq!(services["nats"]["command"][4], "8222");
    assert_eq!(services["nats"]["deploy"]["resources"]["limits"]["memory"], "250000k");
    assert_eq!(services["rustfs"]["entrypoint"][0], "sh");
    assert_eq!(services["rustfs"]["environment"]["RUSTFS_ACCESS_KEY"], "et-storage");
    assert_eq!(services["rustfs"]["deploy"]["resources"]["limits"]["memory"], "500000k");
    let volumes = compose["volumes"].as_mapping().unwrap();
    let names: Vec<&str> = volumes.keys().map(|key| key.as_str().unwrap()).collect();
    assert_eq!(names, ["nats-data", "openobserve-data", "rustfs-data"]);
}

#[test]
fn compose_quotes_values_yaml_would_not_read_as_strings() {
    let input = format!("{CLUSTER}{SERVICES}").replace(
        "    runner: \"wasi\"\n",
        "    runner: \"wasi\"\n    env:\n      FLAG: \"yes\"\n      COUNT: \"3\"\n      NAME: \"plain\"\n",
    );
    let (_root, dir) = generated(&input);
    let compose = read(&dir, "compose.yaml");
    assert!(compose.contains("      FLAG: \"yes\"\n"));
    assert!(compose.contains("      COUNT: \"3\"\n"));
    assert!(compose.contains("      NAME: plain\n"));
    assert!(compose.contains("      - \"8222\"\n"));
}

#[test]
fn k3s_pins_a_hub_to_each_machine_and_the_shared_services_to_the_server() {
    let (_root, dir) = generated(&format!("{CLUSTER}{SERVICES}"));
    let containers = k3s_containers(&dir);
    assert_eq!(containers["ws-server"].1, "server");
    assert_eq!(containers["ws-server-edge"].1, "edge");
    assert_eq!(containers["nats"].1, "server");
    assert_eq!(containers["rustfs"].1, "server");
}

#[test]
fn k3s_hubs_reach_the_shared_services_by_their_service_names() {
    let (_root, dir) = generated(&format!("{CLUSTER}{SERVICES}"));
    let hub_env = container_env(&k3s_containers(&dir)["ws-server-edge"].0);
    assert_eq!(hub_env["WS_BACKPLANE"], "nats://nats:4222");
    // Composed from the service name rather than written out, so link checking does not read an in-cluster DNS name
    // as a link it should be able to reach.
    let storage_service = "rustfs";
    assert_eq!(hub_env["AWS_ENDPOINT"], format!("http://{storage_service}:9000"));
    assert_eq!(hub_env["STORAGE_URL"], "s3://et-storage");
    let names: Vec<&String> = hub_env.keys().collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
}

#[test]
fn k3s_runners_join_their_own_machines_hub_or_the_servers() {
    let (_root, dir) = generated(&format!("{CLUSTER}{SERVICES}"));
    let containers = k3s_containers(&dir);
    let url = |name: &str| container_env(&containers[name].0)["WS_SERVER_URL"].clone();
    assert_eq!(url("math1-twin"), hub_service_ws_url_named("ws-server-edge"));
    assert_eq!(url("math1-trigger"), hub_service_ws_url());
    assert_eq!(url("wasi-twin"), hub_service_ws_url());
}

#[test]
fn k3s_runs_nats_from_its_own_command_over_a_mounted_config() {
    let (_root, dir) = generated(&format!("{CLUSTER}{SERVICES}"));
    let nats = k3s_containers(&dir)["nats"].0.clone();
    assert!(nats.args.is_none());
    assert!(nats.command.is_none());
    let mounts: Vec<String> = nats
        .volume_mounts
        .unwrap()
        .into_iter()
        .map(|mount| mount.mount_path)
        .collect();
    assert_eq!(mounts, ["/data", "/etc/nats"]);
    assert_eq!(
        k3s_documents(&dir, "ConfigMap")["nats-config"]["data"]["nats-server.conf"],
        "port: 4222\nmonitor_port: 8222\njetstream {\n  store_dir: /data\n}\n"
    );
    let readiness = nats.readiness_probe.unwrap().http_get.unwrap();
    assert_eq!(readiness.path.as_deref(), Some("/healthz?js-enabled-only=true"));
}

#[test]
fn k3s_gives_the_shared_services_claims_and_services_in_place_of_hub_storage() {
    let (_root, dir) = generated(&format!("{CLUSTER}{SERVICES}"));
    let rustfs = k3s_containers(&dir)["rustfs"].0.clone();
    assert_eq!(rustfs.command.unwrap()[0], "sh");
    assert!(rustfs.env_from.is_some());
    let claims = k3s_documents(&dir, "PersistentVolumeClaim");
    assert!(claims.contains_key("nats-data"));
    assert!(claims.contains_key("rustfs-data"));
    assert!(!claims.contains_key("ws-server-storage"));
    let services = k3s_documents(&dir, "Service");
    assert!(services.contains_key("ws-server-edge"));
    assert_eq!(services["nats"]["spec"]["ports"][1]["port"].as_u64(), Some(8_222_u64));
}

#[test]
fn the_readme_says_which_machine_runs_what_and_ships_the_hub_image_to_each() {
    let (_root, dir) = generated(&format!("{CLUSTER}{SERVICES}"));
    let readme = read(&dir, "README.md");
    assert!(readme.contains("OpenObserve, OTLP and object store credentials"));
    assert!(readme.contains("`server` runs OpenObserve, `ws-server`, and the NATS server and object store the hubs"));
    assert!(readme.contains("Each runner joins the hub on its own\nmachine, or the one on `server`"));
    assert!(readme.contains("docker save \"$image\" | ssh 10.0.0.1 sudo k3s ctr images import -\n"));
    assert!(readme.contains("docker save \"$image\" | ssh 10.0.0.2 sudo k3s ctr images import -\n"));
    assert!(!readme.contains("docker save \"$image\" | ssh 10.0.0.3 "));
}
