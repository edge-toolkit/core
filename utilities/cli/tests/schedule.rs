//! Covers a scheduled agent: how `schedule:` is validated, and how each format runs it or refuses it.
//!
//! The fixture is the math1 pair with the trigger on a schedule, as `verification/local/input/math1-cron.yaml` has it,
//! so every check below compares a scheduled runner against an unscheduled one in the same output.
#![cfg(test)]

use std::path::{Path, PathBuf};

use et_cli::{ClusterInput, OutputType, Schedule, generate_deployment, regenerate_verification};
use fs_err as fs;
use k8s_openapi::api::batch::v1::CronJob;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use serde::Deserialize;
use tempfile::{TempDir, tempdir};

/// Two machines, with the scheduled trigger on `edge` and the twin on the server.
const MACHINES: &str = r#"machines:
  - name: "server"
    address: "10.0.0.1"
    agents: ["math1-twin"]
  - name: "edge"
    address: "10.0.0.2"
    agents: ["math1-trigger"]
"#;

/// The math1 pair, with `{schedule}` spliced into the trigger.
fn scenario(schedule: &str) -> String {
    format!(
        r#"cluster_name: "timed"
agents:
  - name: "math1-twin"
    runner: "web"
    resources:
      - type: "math1"
  - name: "math1-trigger"
    runner: "web"
{schedule}    resources:
      - type: "math1-sender"
"#
    )
}

/// A `schedule:` block for the trigger.
fn every(cron: &str, time_zone: &str) -> String {
    format!("    schedule:\n      cron: \"{cron}\"\n      time_zone: \"{time_zone}\"\n")
}

/// What generating `input` as `output_type` into a fresh directory returns, and that directory.
fn generate(input: &str, output_type: OutputType) -> (TempDir, PathBuf, String) {
    let test_root = tempdir().unwrap();
    let input_file = test_root.path().join("timed.yaml");
    fs::write(&input_file, input).unwrap();
    let output_dir = test_root.path().join("output");
    let outcome = generate_deployment(&input_file, &output_dir, Some(output_type))
        .map_or_else(|error| error.to_string(), |_summary| String::default());
    (test_root, output_dir, outcome)
}

/// The directory generating `input` as `output_type` succeeds in writing, with the root kept alive.
fn generated(input: &str, output_type: OutputType) -> (TempDir, PathBuf) {
    let (test_root, output_dir, outcome) = generate(input, output_type);
    assert_eq!(outcome, "");
    (test_root, output_dir)
}

/// One generated YAML file, parsed.
fn yaml(dir: &Path, file: &str) -> serde_yaml::Value {
    serde_yaml::from_str(&fs::read_to_string(dir.join(file)).unwrap()).unwrap()
}

/// Every document of the generated `k3s.yaml`, in order.
fn k3s_documents(dir: &Path) -> Vec<serde_yaml::Value> {
    let manifest = fs::read_to_string(dir.join("k3s.yaml")).unwrap();
    serde_yaml::Deserializer::from_str(&manifest)
        .map(serde_yaml::Value::deserialize)
        .collect::<Result<_, _>>()
        .unwrap()
}

/// What every Kubernetes object carries, whatever its kind.
#[derive(Deserialize)]
struct Object {
    kind: String,
    metadata: ObjectMeta,
}

/// The names of the k3s documents of `kind`.
fn k3s_names(dir: &Path, kind: &str) -> Vec<String> {
    k3s_documents(dir)
        .into_iter()
        .map(|document| serde_yaml::from_value::<Object>(document).unwrap())
        .filter(|object| object.kind == kind)
        .map(|object| object.metadata.name.unwrap())
        .collect()
}

/// The k3s `CronJob` for the trigger.
fn cron_job(dir: &Path) -> CronJob {
    let document = k3s_documents(dir)
        .into_iter()
        .find(|document| document["kind"] == "CronJob");
    serde_yaml::from_value(document.unwrap()).unwrap()
}

/// The generated README.
fn readme(dir: &Path) -> String {
    fs::read_to_string(dir.join("README.md")).unwrap()
}

#[test]
fn a_schedule_defaults_to_unset_and_is_left_out_when_unset() {
    let cluster: ClusterInput = serde_yaml::from_str(&scenario("")).unwrap();
    assert!(cluster.agents[1].schedule.is_unset());
    assert!(Schedule::default().is_unset());
    assert!(!serde_yaml::to_string(&cluster).unwrap().contains("schedule"));

    let cluster: ClusterInput = serde_yaml::from_str(&scenario(&every("*/5 * * * *", "UTC"))).unwrap();
    assert_eq!(cluster.agents[1].schedule.cron, "*/5 * * * *");
    assert_eq!(cluster.agents[1].schedule.time_zone, "UTC");

    let unknown: Result<ClusterInput, _> =
        serde_yaml::from_str(&scenario("    schedule:\n      cron: \"* * * * *\"\n      every: 5\n"));
    assert!(unknown.unwrap_err().to_string().contains("every"));
}

#[test]
fn only_the_cron_grammar_every_scheduler_shares_is_accepted() {
    for accepted in [
        "* * * * *",
        "*/15 0-6,22 1 * *",
        "0 9 * jan-mar MON-FRI",
        "@hourly",
        "@daily",
        " 0 0 * * 0 ",
        "0 0 * * */7",
        "0 0 * * 1-5/2",
    ] {
        let (_root, _dir, outcome) = generate(&scenario(&every(accepted, "")), OutputType::K3s);
        assert_eq!(outcome, "");
    }
    for (rejected, reason) in [
        ("0 * * * * *", "has 6 fields"),
        ("* * * *", "has 4 fields"),
        ("0 0 L * *", "\"L\" is not a number"),
        ("0 0 * * 5#3", "\"5#3\" is not a number"),
        ("0 0 ? * *", "\"?\" is not a number"),
        ("0 0 * MON *", "\"MON\" is not a number"),
        ("*/ * * * *", "\"\" is not a number"),
        ("0 0 * * 7", "day of week 7 is past 6"),
        ("0 0 * * 5-7", "day of week 7 is past 6"),
        ("0 0 * * 1/*", "\"*\" is not a number"),
        ("@every 5m", "the only shorthands accepted are @yearly"),
        ("61 * * * *", "math1-trigger"),
    ] {
        let (_root, _dir, outcome) = generate(&scenario(&every(rejected, "")), OutputType::K3s);
        assert!(outcome.contains("is not a standard cron expression"));
        assert!(outcome.contains(reason));
    }
}

#[test]
fn an_unknown_or_misspelt_time_zone_is_refused() {
    let (_root, _dir, outcome) = generate(&scenario(&every("* * * * *", "Mars/Olympus")), OutputType::K3s);
    assert!(outcome.contains("\"Mars/Olympus\", which is not an IANA time zone name"));

    let (_root, _dir, outcome) = generate(&scenario(&every("* * * * *", "australia/perth")), OutputType::K3s);
    assert!(outcome.contains("\"australia/perth\", which the schedulers only know as \"Australia/Perth\""));
}

#[test]
fn a_schedule_written_out_with_no_cron_is_refused_rather_than_run_continuously() {
    for cron in ["\"\"", "\"  \""] {
        let (_root, _dir, outcome) = generate(
            &scenario(&format!("    schedule:\n      cron: {cron}\n")),
            OutputType::K3s,
        );
        assert!(outcome.contains("`cron` is empty; leave `schedule:` out for an agent that runs continuously"));
    }
}

#[test]
fn a_schedule_needs_a_runner_to_start() {
    let input = r#"cluster_name: "timed"
agents:
  - name: "browser-only"
    schedule:
      cron: "* * * * *"
    resources:
      - type: "math1"
"#;
    let (_root, _dir, outcome) = generate(input, OutputType::K3s);
    assert!(outcome.contains("\"browser-only\" sets a schedule but names no runner"));
}

#[test]
fn mise_and_dekit_refuse_a_scheduled_agent_and_write_nothing() {
    for output_type in [OutputType::Mise, OutputType::Dekit] {
        let (_root, output_dir, outcome) = generate(&scenario(&every("* * * * *", "")), output_type);
        let refused = format!(
            "\"math1-trigger\" sets a schedule, which {} cannot run",
            output_type.name()
        );
        assert!(outcome.contains(&refused));
        assert!(outcome.ends_with("use k3s, docker-compose or process-compose"));
        assert!(!output_dir.exists());
        assert!(!output_type.runs_schedules());
    }
}

#[test]
fn process_compose_alone_still_writes_the_mise_toml_it_runs() {
    let (_root, dir) = generated(&scenario(&every("* * * * *", "")), OutputType::ProcessCompose);
    let processes = &yaml(&dir, "process-compose.yaml")["processes"];
    assert_eq!(processes["math1-trigger"]["command"], "mise run math1-trigger");
    let mise = fs::read_to_string(dir.join("mise.toml")).unwrap();
    assert!(mise.contains("[tasks.math1-trigger]"));
    assert!(mise.contains("\"aqua:F1bonacc1/process-compose\" = \"latest\""));
    assert!(readme(&dir).contains("Files: `mise.toml`, `process-compose.yaml`."));
    assert!(!dir.join("dekit.yaml").exists());
}

#[test]
fn regeneration_renders_a_scheduled_scenario_only_where_it_can_be_scheduled() {
    let test_root = tempdir().unwrap();
    let verification = test_root.path().join("verification");
    let input = verification.join("local/input/timed.yaml");
    fs::create_dir_all(input.parent().unwrap()).unwrap();
    fs::write(&input, scenario(&every("* * * * *", ""))).unwrap();
    let dir = verification.join("local/output/timed");

    let _only_dekit = regenerate_verification(&verification, Some(OutputType::Dekit)).unwrap();
    assert!(!dir.exists());

    let _every_format = regenerate_verification(&verification, None).unwrap();
    for file in ["compose.yaml", "k3s.yaml", "process-compose.yaml", "mise.toml"] {
        assert!(dir.join(file).exists());
    }
    assert!(!dir.join("dekit.yaml").exists());
    assert!(readme(&dir).contains("Files: `mise.toml`, `process-compose.yaml`, `compose.yaml`, `k3s.yaml`."));
    assert!(!readme(&dir).contains("## Run With Mise"));
}

#[test]
fn k3s_runs_a_scheduled_runner_as_a_forbid_cron_job() {
    let (_root, dir) = generated(&scenario(&every("*/10 * * * *", "Australia/Perth")), OutputType::K3s);
    assert_eq!(k3s_names(&dir, "CronJob"), ["math1-trigger"]);
    assert!(!k3s_names(&dir, "Deployment").contains(&"math1-trigger".to_owned()));
    assert!(k3s_names(&dir, "Deployment").contains(&"math1-twin".to_owned()));

    let spec = cron_job(&dir).spec.unwrap();
    assert_eq!(spec.schedule, "*/10 * * * *");
    assert_eq!(spec.time_zone.as_deref(), Some("Australia/Perth"));
    assert_eq!(spec.concurrency_policy.as_deref(), Some("Forbid"));
    let pod = spec.job_template.spec.unwrap().template.spec.unwrap();
    assert_eq!(pod.restart_policy.as_deref(), Some("Never"));
    assert_eq!(pod.containers[0].name, "math1-trigger");
    assert!(pod.node_selector.is_none());

    let (_root, dir) = generated(
        &format!("{}{MACHINES}", scenario(&every("@hourly", ""))),
        OutputType::K3s,
    );
    let spec = cron_job(&dir).spec.unwrap();
    assert!(spec.time_zone.is_none());
    let pod = spec.job_template.spec.unwrap().template.spec.unwrap();
    assert_eq!(pod.node_selector.unwrap()["edge-toolkit/machine"], "edge");
}

#[test]
fn compose_has_ofelia_restart_the_scheduled_runner_container() {
    let compose = OutputType::DockerCompose;
    let (_root, dir) = generated(&scenario(&every("*/10 * * * *", "Australia/Perth")), compose);
    let services = &yaml(&dir, "compose.yaml")["services"];
    let trigger = &services["math1-trigger"];
    assert_eq!(trigger["container_name"], "et-timed-math1-trigger");
    assert_eq!(trigger["labels"]["edge-toolkit/scenario"], "timed");
    assert!(trigger["restart"].is_null());
    assert!(services["math1-twin"]["container_name"].is_null());
    assert!(services["math1-twin"]["labels"].is_null());
    assert!(readme(&dir).contains("### Scheduled Agents With Docker Compose"));

    let (_root, dir) = generated(&scenario(""), compose);
    assert!(yaml(&dir, "compose.yaml")["services"]["scheduler"].is_null());
    assert!(!readme(&dir).contains("Scheduled Agents"));
}

#[test]
fn the_compose_scheduler_declares_each_job_as_its_own_labels() {
    let compose = OutputType::DockerCompose;
    let (_root, dir) = generated(&scenario(&every("*/10 * * * *", "Australia/Perth")), compose);
    let scheduler = &yaml(&dir, "compose.yaml")["services"]["scheduler"];
    assert_eq!(scheduler["image"], "ghcr.io/netresearch/ofelia:1.0.1");
    assert_eq!(scheduler["command"][3], "label=edge-toolkit/scenario=timed");
    assert_eq!(scheduler["volumes"][0], "/var/run/docker.sock:/var/run/docker.sock:ro");
    let labels = &scheduler["labels"];
    assert_eq!(labels["edge-toolkit/scenario"], "timed");
    assert_eq!(labels["ofelia.enabled"], "true");
    assert_eq!(
        labels["ofelia.job-run.math1-trigger.schedule"],
        "CRON_TZ=Australia/Perth */10 * * * *"
    );
    assert_eq!(
        labels["ofelia.job-run.math1-trigger.container"],
        "et-timed-math1-trigger"
    );
    assert_eq!(labels["ofelia.job-run.math1-trigger.no-overlap"], "true");
    assert!(scheduler["profiles"].is_null());
}

#[test]
fn compose_runs_a_scheduler_on_each_machine_with_a_scheduled_runner() {
    let input = format!("{}{MACHINES}", scenario(&every("* * * * *", "")));
    let (_root, dir) = generated(&input, OutputType::DockerCompose);
    let services = &yaml(&dir, "compose.yaml")["services"];
    assert!(services["scheduler"].is_null());
    let scheduler = &services["scheduler-edge"];
    assert_eq!(scheduler["profiles"][0], "edge");
    assert_eq!(
        scheduler["labels"]["ofelia.job-run.math1-trigger.schedule"],
        "* * * * *"
    );
    // A remote runner is otherwise restarted until the hub answers; a scheduled one waits for its next tick instead.
    assert!(services["math1-trigger"]["restart"].is_null());
}

#[test]
fn process_compose_schedules_the_runner_and_orders_the_rest() {
    let process_compose = OutputType::ProcessCompose;
    let (_root, dir) = generated(&scenario(&every("*/10 * * * *", "UTC")), process_compose);
    let processes = &yaml(&dir, "process-compose.yaml")["processes"];
    assert_eq!(processes["math1-trigger"]["schedule"]["cron"], "*/10 * * * *");
    assert_eq!(processes["math1-trigger"]["schedule"]["timezone"], "UTC");
    assert!(processes["math1-twin"]["schedule"].is_null());
    assert!(processes["math1-twin"]["namespace"].is_null());

    let (_root, dir) = generated(
        &format!("{}{MACHINES}", scenario(&every("@daily", ""))),
        process_compose,
    );
    let processes = &yaml(&dir, "process-compose.yaml")["processes"];
    assert_eq!(processes["math1-trigger"]["namespace"], "edge");
    assert_eq!(processes["ws-server"]["namespace"], "server");
    assert!(processes["math1-trigger"]["schedule"]["timezone"].is_null());
    // The scheduled remote runner is not restarted; an unscheduled one on another machine would be.
    assert!(processes["math1-trigger"]["availability"].is_null());
    assert!(processes["math1-trigger"]["depends_on"].is_null());
    assert!(readme(&dir).contains("process-compose up --no-server -f process-compose.yaml -n edge"));
}

#[test]
fn process_compose_starts_each_process_once_what_it_talks_to_is_ready() {
    let (_root, dir) = generated(&scenario(""), OutputType::ProcessCompose);
    let processes = &yaml(&dir, "process-compose.yaml")["processes"];
    assert_eq!(
        processes["ws-server"]["depends_on"]["openobserve"]["condition"],
        "process_healthy"
    );
    assert_eq!(
        processes["math1-twin"]["depends_on"]["ws-server"]["condition"],
        "process_healthy"
    );
    assert_eq!(processes["ws-server"]["readiness_probe"]["http_get"]["port"], 8080_u16);
    assert_eq!(
        processes["openobserve"]["shutdown"]["command"],
        "docker stop openobserve-timed"
    );
    assert_eq!(processes["math1-twin"]["shutdown"]["signal"], 2_u8);
    let disabled = processes
        .as_mapping()
        .unwrap()
        .values()
        .filter(|process| process["disabled"].as_bool().unwrap_or_default());
    assert_eq!(disabled.count(), 1);
}

#[test]
fn an_unscheduled_remote_runner_restarts_until_the_hub_answers() {
    let (_root, dir) = generated(&format!("{}{MACHINES}", scenario("")), OutputType::ProcessCompose);
    let processes = &yaml(&dir, "process-compose.yaml")["processes"];
    assert_eq!(processes["math1-trigger"]["availability"]["restart"], "on_failure");
}

#[test]
fn every_format_refuses_a_runner_it_has_no_binary_for() {
    let input = scenario("").replace(
        "runner: \"web\"\n    resources:\n      - type: \"math1\"",
        "runner: \"nope\"\n    resources:\n      - type: \"math1\"",
    );
    for output_type in OutputType::ALL {
        let (_root, _dir, outcome) = generate(&input, *output_type);
        assert!(outcome.contains("asks for runner \"nope\""));
    }
}

#[test]
fn process_compose_reports_a_file_it_cannot_write() {
    let test_root = tempdir().unwrap();
    let input_file = test_root.path().join("timed.yaml");
    fs::write(&input_file, scenario("")).unwrap();
    let output_dir = test_root.path().join("output");
    // A directory where the file belongs, so the write is refused by the filesystem itself on every platform.
    fs::create_dir_all(output_dir.join("process-compose.yaml")).unwrap();

    let refused = generate_deployment(&input_file, &output_dir, Some(OutputType::ProcessCompose)).unwrap_err();
    assert!(refused.to_string().contains("process-compose.yaml"));
}

#[test]
fn a_runner_cannot_take_a_scheduler_service_name() {
    let (_root, _dir, outcome) = generate(&scenario("").replace("math1-trigger", "scheduler"), OutputType::K3s);
    assert!(outcome.contains("would create runner \"scheduler\", a name the generated deployment already uses"));

    let input = format!("{}{MACHINES}", scenario("")).replace("math1-trigger", "scheduler-edge");
    let (_root, _dir, outcome) = generate(&input, OutputType::K3s);
    assert!(outcome.contains("would create runner \"scheduler-edge\""));
}
