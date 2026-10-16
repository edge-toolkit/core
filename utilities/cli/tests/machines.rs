//! Covers a scenario that spans machines: how `machines:` is validated, and how each format splits along it.
//!
//! The fixture splits the math1 pair the way `verification/local/input/math1-multi.yaml` does -- the trigger on the
//! server with the hub, the twin on `edge` -- plus a third agent no machine lists, which has to land on the server.
//! The single-host half of each test is what keeps the option opt-in: a scenario without `machines:` must render
//! none of it.
#![cfg(test)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use et_cli::{ClusterInput, OutputType, generate_deployment, hub_ws_url, hub_ws_url_at, regenerate_verification};
use fs_err as fs;
use k8s_openapi::api::apps::v1::Deployment;
use serde::Deserialize as _;
use tempfile::{TempDir, tempdir};

/// The agents every scenario here declares, with `{machines}` spliced in after them.
const AGENTS: &str = r#"cluster_name: "spread"
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
"#;

/// Two machines, with `wasi-twin` listed on neither.
const MACHINES: &str = r#"machines:
  - name: "server"
    address: "10.0.0.1"
    agents: ["math1-trigger"]
  - name: "edge"
    address: "10.0.0.2"
    agents: ["math1-twin"]
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

/// Each k3s `Deployment` by name, with the `nodeSelector` its pods carry, empty when they carry none.
fn node_selectors(dir: &Path) -> Vec<(String, BTreeMap<String, String>)> {
    let manifest = read(dir, "k3s.yaml");
    serde_yaml::Deserializer::from_str(&manifest)
        .map(|document| serde_yaml::Value::deserialize(document).unwrap())
        .filter(|document| document["kind"] == "Deployment")
        .map(|document| {
            let deployment: Deployment = serde_yaml::from_value(document).unwrap();
            let pod = deployment.spec.unwrap().template.spec.unwrap();
            (deployment.metadata.name.unwrap(), pod.node_selector.unwrap_or_default())
        })
        .collect()
}

fn yaml(dir: &Path, file: &str) -> serde_yaml::Value {
    serde_yaml::from_str(&read(dir, file)).unwrap()
}

/// The error generating `machines` alongside the shared agents produces.
fn error_for(machines: &str) -> String {
    let test_root = tempdir().unwrap();
    let input_file = test_root.path().join("cluster.yaml");
    fs::write(&input_file, format!("{AGENTS}{machines}")).unwrap();
    generate_deployment(&input_file, &test_root.path().join("output"), None)
        .unwrap_err()
        .to_string()
}

fn one_machine(name: &str, address: &str, agents: &str) -> String {
    format!("machines:\n  - name: \"{name}\"\n    address: \"{address}\"\n    agents: [{agents}]\n")
}

#[test]
fn machines_default_to_none_and_are_left_out_when_there_are_none() {
    let cluster: ClusterInput = serde_yaml::from_str(AGENTS).unwrap();
    assert!(cluster.machines.is_empty());
    assert!(!serde_yaml::to_string(&cluster).unwrap().contains("machines"));
    assert!(ClusterInput::default().machines.is_empty());

    let cluster: ClusterInput = serde_yaml::from_str(&format!("{AGENTS}{MACHINES}")).unwrap();
    assert_eq!(cluster.machines.len(), 2);
    assert_eq!(cluster.machines[1].name, "edge");
    assert_eq!(cluster.machines[1].address, "10.0.0.2");
    assert_eq!(cluster.machines[1].agents, ["math1-twin"]);
}

#[test]
fn a_remote_runner_reaches_the_hub_at_the_server_address() {
    // Only the host differs from the URL a runner beside the hub uses: the same scheme, port and path.
    assert_eq!(hub_ws_url_at("10.0.0.1"), hub_ws_url().replace("localhost", "10.0.0.1"));
    assert_eq!(hub_ws_url(), hub_ws_url_at("localhost"));
}

#[test]
fn a_machine_name_must_be_an_rfc_1123_label() {
    let error = error_for(&one_machine("Edge", "10.0.0.1", ""));
    assert!(error.contains("Machine \"Edge\" is not an RFC 1123 label"));
}

#[test]
fn a_machine_address_must_be_a_host_name_or_ipv4_address() {
    let error = error_for(&one_machine("server", "", ""));
    assert!(error.contains("it is empty"));
    let error = error_for(&one_machine("server", &"a".repeat(254), ""));
    assert!(error.contains("longer than 253"));
    let error = error_for(&one_machine("server", "host;reboot", ""));
    assert!(error.contains("it contains ';'"));
    // Handed to `ssh` as its destination, where a leading `-` would be read as an option.
    let error = error_for(&one_machine("server", "-oproxycommand", ""));
    assert!(error.contains("does not start with a letter or digit"));
}

#[test]
fn a_machine_name_is_declared_once() {
    let error = error_for(&format!(
        "{}{}",
        one_machine("server", "10.0.0.1", ""),
        one_machine("server", "10.0.0.2", "").replace("machines:\n", "")
    ));
    assert!(error.contains("Machine \"server\" is declared more than once"));
}

#[test]
fn a_machine_lists_only_agents_the_scenario_declares() {
    let error = error_for(&one_machine("server", "10.0.0.1", "\"camera\""));
    assert!(error.contains("Machine \"server\" lists agent \"camera\""));
}

#[test]
fn an_agent_runs_on_one_machine() {
    let error = error_for(&format!(
        "{}{}",
        one_machine("server", "10.0.0.1", "\"math1-twin\""),
        one_machine("edge", "10.0.0.2", "\"math1-twin\"").replace("machines:\n", "")
    ));
    assert!(error.contains("\"math1-twin\" is listed on both machine \"server\" and machine \"edge\""));
}

#[test]
fn a_runner_cannot_take_the_name_of_a_machine_task() {
    // `machine-edge` is the mise task that starts the edge machine's share, so a runner under that name replaces it.
    let test_root = tempdir().unwrap();
    let input_file = test_root.path().join("cluster.yaml");
    let agent = "  - name: \"machine-edge\"\n    runner: \"web\"\n    resources:\n      - type: \"math1\"\n";
    let input = format!("{AGENTS}{agent}{MACHINES}");
    fs::write(&input_file, input).unwrap();
    let error = generate_deployment(&input_file, &test_root.path().join("output"), None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("\"machine-edge\", a name the generated deployment already uses"));
}

#[test]
fn k3s_pins_each_deployment_to_its_machine() {
    let (_root, dir) = generated(&format!("{AGENTS}{MACHINES}"));
    let selectors = node_selectors(&dir);
    let machine_of = |name: &str| {
        let (_, selector) = selectors.iter().find(|(found, _)| found == name).unwrap();
        selector["edge-toolkit/machine"].clone()
    };

    // The collector and the hub mount node-local claims, so they stay on the server with the unlisted agent.
    assert_eq!(machine_of("openobserve"), "server");
    assert_eq!(machine_of("ws-server"), "server");
    assert_eq!(machine_of("math1-trigger"), "server");
    assert_eq!(machine_of("wasi-twin"), "server");
    assert_eq!(machine_of("math1-twin"), "edge");

    let (_root, dir) = generated(AGENTS);
    assert!(node_selectors(&dir).iter().all(|(_, selector)| selector.is_empty()));
}

#[test]
fn compose_gives_each_machine_a_profile() {
    let (_root, dir) = generated(&format!("{AGENTS}{MACHINES}"));
    let services = &yaml(&dir, "compose.yaml")["services"];
    for server_side in [
        "openobserve",
        "ws-server-hub",
        "ws-server",
        "math1-trigger",
        "wasi-twin",
    ] {
        assert_eq!(services[server_side]["profiles"][0], "server");
    }

    let twin = &services["math1-twin"];
    assert_eq!(twin["profiles"][0], "edge");
    assert_eq!(
        twin["environment"]["WS_SERVER_URL"].as_str(),
        Some(hub_ws_url_at("10.0.0.1").as_str())
    );
    assert_eq!(twin["restart"], "on-failure");
    assert_eq!(twin["depends_on"]["ws-server"]["required"].as_bool(), Some(false));

    // A runner on the server keeps the healthy-hub gate and the hub's own host.
    let trigger = &services["math1-trigger"];
    assert_eq!(
        trigger["environment"]["WS_SERVER_URL"].as_str(),
        Some(hub_ws_url().as_str())
    );
    assert!(trigger["restart"].is_null());
    assert!(trigger["depends_on"]["ws-server"]["required"].is_null());

    let (_root, dir) = generated(AGENTS);
    let single = read(&dir, "compose.yaml");
    assert!(!single.contains("profiles:") && !single.contains("restart:") && !single.contains("required:"));
}

#[test]
fn mise_starts_each_machine_share_from_its_own_task() {
    let (_root, dir) = generated(&format!("{AGENTS}{MACHINES}"));
    let mise: toml::Table = toml::from_str(&read(&dir, "mise.toml")).unwrap();
    let tasks = &mise["tasks"];
    let depends = |task: &str| -> Vec<String> {
        tasks[task]["depends"]
            .as_array()
            .unwrap()
            .iter()
            .map(|name| name.as_str().unwrap().to_string())
            .collect()
    };

    assert_eq!(
        depends("machine-server"),
        ["math1-trigger", "openobserve", "wasi-twin", "ws-server"]
    );
    assert_eq!(depends("machine-edge"), ["math1-twin"]);
    // Everything on one host stays available alongside them.
    assert_eq!(depends("generated-scenario").len(), 5);
    assert_eq!(
        tasks["math1-twin"]["env"]["WS_SERVER_URL"].as_str(),
        Some(hub_ws_url_at("10.0.0.1").as_str())
    );
    assert_eq!(
        tasks["math1-trigger"]["env"]["WS_SERVER_URL"].as_str(),
        Some(hub_ws_url().as_str())
    );

    let (_root, dir) = generated(AGENTS);
    assert!(!read(&dir, "mise.toml").contains("machine-"));
}

#[test]
fn dekit_tags_each_task_with_its_machine_and_starts_none_on_up() {
    let (_root, dir) = generated(&format!("{AGENTS}{MACHINES}"));
    let tasks = &yaml(&dir, "dekit.yaml")["tasks"];
    // Every task but the edge twin is the server's, the hub's own and the unlisted agent's alike.
    for (task, body) in tasks.as_mapping().unwrap() {
        let machine = if task == "math1-twin" { "edge" } else { "server" };
        assert_eq!(body["tags"][0], machine);
        assert!(body["autostart"].is_null());
    }
    // The hub is on another machine, so the edge twin restarts until it answers rather than waiting on a task.
    assert!(tasks["math1-twin"]["deps"].is_null());
    assert_eq!(tasks["math1-twin"]["autorestart"], "on-failure");
    assert_eq!(tasks["math1-trigger"]["deps"][0], "ws-server");
    assert!(tasks["math1-trigger"]["autorestart"].is_null());

    let (_root, dir) = generated(AGENTS);
    let single = &yaml(&dir, "dekit.yaml")["tasks"];
    assert!(single["math1-twin"]["tags"].is_null());
    assert_eq!(single["math1-twin"]["autostart"].as_bool(), Some(true));
}

#[test]
fn the_readme_says_how_to_run_each_machine_share() {
    let (_root, dir) = generated(&format!("{AGENTS}{MACHINES}"));
    let readme = read(&dir, "README.md");

    assert_eq!(
        readme.matches("### Run Across Machines With").count(),
        OutputType::ALL.len()
    );
    assert!(readme.contains("mise run machine-edge"));
    assert!(readme.contains("docker compose --profile edge up --build"));
    assert!(readme.contains("dekit start +edge"));
    // Every format's section shares the one file, so a heading repeated between them is markdownlint's MD024.
    let headings: Vec<&str> = readme.lines().filter(|line| line.starts_with('#')).collect();
    let unique: std::collections::BTreeSet<&str> = headings.iter().copied().collect();
    assert_eq!(headings.len(), unique.len());

    let (_root, dir) = generated(AGENTS);
    assert!(!read(&dir, "README.md").contains("Run Across Machines"));
}

#[test]
fn the_k3s_readme_joins_the_machines_and_imports_each_image_where_it_runs() {
    let (_root, dir) = generated(&format!("{AGENTS}{MACHINES}"));
    let readme = read(&dir, "README.md");

    assert!(readme.contains("sudo k3s server --node-label edge-toolkit/machine=server\n"));
    assert!(readme.contains("--token \"$token\" --node-label edge-toolkit/machine=edge\n"));
    assert!(readme.contains("server=10.0.0.1\n"));
    // The scenario image goes to the server alone, and each runner image to the machines running that kind.
    assert!(readme.contains("docker save \"$image\" | ssh 10.0.0.1 sudo k3s ctr images import -"));
    assert!(readme.contains("docker save et-ws-web-runner:latest | ssh 10.0.0.1 sudo k3s ctr images import -"));
    assert!(readme.contains("docker save et-ws-wasi-runner:latest | ssh 10.0.0.1 sudo k3s ctr images import -"));
    assert!(readme.contains("docker save et-ws-web-runner:latest | ssh 10.0.0.2 sudo k3s ctr images import -"));
    assert!(!readme.contains("et-ws-wasi-runner:latest | ssh 10.0.0.2"));

    // A published cluster pulls its runner images, so only the scenario image is imported anywhere.
    let (_root, dir) = generated(&format!("{AGENTS}artifact_source: \"published\"\n{MACHINES}"));
    let published = read(&dir, "README.md");
    assert!(published.contains("docker save \"$image\" | ssh 10.0.0.1"));
    assert!(!published.contains("runner:latest | ssh"));
}
