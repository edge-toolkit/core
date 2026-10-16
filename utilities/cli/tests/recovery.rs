//! Recovery scenarios generate deployable, scoped supervision without changing scenarios that do not request it.
#![cfg(test)]

use std::path::{Path, PathBuf};

use et_cli::{OutputType, RecoverySettings, generate_deployment};
use et_recovery_supervisor::config::{EnvConfig, Mode};
use fs_err as fs;
use serde::Deserialize as _;
use serde_json::{Value, json};

#[derive(serde::Deserialize)]
struct ResourceHeader {
    metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta,
}

/// Read the Kubernetes metadata name through a typed resource header.
fn object_name(object: &Value) -> String {
    let header: ResourceHeader = serde_json::from_value(object.clone()).unwrap();
    header.metadata.name.unwrap()
}

/// Load the checked-in recovery scenario used by deployment-generation tests.
fn scenario() -> Value {
    let file = edge_toolkit::config::get_project_root().join("verification/local/input/pyo3-math1-recovery.yaml");
    serde_yaml::from_str(&fs::read_to_string(file).unwrap()).unwrap()
}

/// Serialize a scenario mutation into an isolated input file.
fn write_input(root: &Path, input: &Value) -> PathBuf {
    let file = root.join("scenario.yaml");
    fs::write(&file, serde_yaml::to_string(input).unwrap()).unwrap();
    file
}

/// Generate recovery manifests and decode every resulting Kubernetes document.
fn generate(input: &Value) -> (tempfile::TempDir, Vec<Value>) {
    let root = tempfile::tempdir().unwrap();
    let file = write_input(root.path(), input);
    let output = root.path().join("output");
    let _summary = generate_deployment(&file, &output, Some(OutputType::K3s)).unwrap();
    let text = fs::read_to_string(output.join("k3s.yaml")).unwrap();
    let objects = serde_yaml::Deserializer::from_str(&text)
        .map(|doc| Value::deserialize(doc).unwrap())
        .collect();
    (root, objects)
}

/// Return the generation failure for an invalid scenario mutation.
fn generation_error(input: &Value) -> et_cli::CliError {
    let root = tempfile::tempdir().unwrap();
    let file = write_input(root.path(), input);
    generate_deployment(&file, &root.path().join("output"), Some(OutputType::K3s)).unwrap_err()
}

/// Find one required Kubernetes document by kind and metadata name.
fn object<'objects>(objects: &'objects [Value], kind: &str, name: &str) -> &'objects Value {
    objects
        .iter()
        .find(|object| object["kind"] == kind && object_name(object) == name)
        .unwrap()
}

/// Extract a deployment's rendered container settings for serde-env round trips.
fn environment(deployment: &Value) -> Vec<(String, String)> {
    deployment["spec"]["template"]["spec"]["containers"][0]["env"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            (
                item["name"].as_str().unwrap().into(),
                item["value"].as_str().unwrap().into(),
            )
        })
        .collect()
}

/// Ensure scenarios without recovery do not acquire a supervisor deployment.
fn assert_no_supervisor(objects: &[Value]) {
    assert!(
        !objects
            .iter()
            .any(|object| object_name(object) == "recovery-supervisor")
    );
}

/// Round-trip generated settings and verify least-privilege in-cluster credentials.
#[test]
fn generates_valid_supervisor_settings_and_scoped_in_cluster_credentials() {
    let (_root, objects) = generate(&scenario());
    let deployment = object(&objects, "Deployment", "recovery-supervisor");
    let env = environment(deployment);
    let config: EnvConfig = serde_env::from_iter(env).unwrap();
    config.recovery.validate().unwrap();
    assert_eq!(config.recovery.context, "in-cluster");
    assert_eq!(config.recovery.mode, Mode::ObserveOnly);
    assert_eq!(config.recovery.namespace, "et-pyo3-math1-recovery");
    assert_eq!(config.recovery.workloads[0].deployment, "example-worker");
    let role = object(&objects, "Role", "recovery-supervisor");
    assert_eq!(
        role["rules"],
        json!([{
            "apiGroups": ["apps"], "resources": ["deployments"],
            "resourceNames": ["example-worker"], "verbs": ["get"]
        }])
    );
    let binding = object(&objects, "RoleBinding", "recovery-supervisor");
    assert_eq!(binding["roleRef"]["kind"], "Role");
    assert_eq!(binding["subjects"][0]["namespace"], "et-pyo3-math1-recovery");
    let credentials = object(&objects, "ConfigMap", "recovery-credentials")["data"]["kubeconfig"]
        .as_str()
        .unwrap();
    let kubeconfig: Value = serde_yaml::from_str(credentials).unwrap();
    assert_eq!(kubeconfig["current-context"], "in-cluster");
    assert_eq!(
        kubeconfig["users"][0]["user"]["tokenFile"],
        "/var/run/secrets/kubernetes.io/serviceaccount/token"
    );
    assert!(kubeconfig["users"][0]["user"]["token"].is_null());
    assert!(
        !objects
            .iter()
            .any(|object| object["kind"] == "ClusterRole" || object["kind"] == "ClusterRoleBinding")
    );
}

/// Preserve recovery state and prevent overlapping supervisor or checkpoint-writer replacements.
#[test]
fn budgets_persist_and_replacements_do_not_overlap_during_rollouts() {
    let (_root, objects) = generate(&scenario());
    let deployment = object(&objects, "Deployment", "recovery-supervisor");
    assert_eq!(deployment["spec"]["replicas"], 1_u32);
    assert_eq!(deployment["spec"]["strategy"]["type"], "Recreate");
    let pod = &deployment["spec"]["template"]["spec"];
    assert_eq!(pod["serviceAccountName"], "recovery-supervisor");
    assert_eq!(pod["automountServiceAccountToken"].as_bool(), Some(true));
    assert_eq!(pod["securityContext"]["fsGroup"], 10_001_i64);
    assert_eq!(
        pod["containers"][0]["securityContext"]["readOnlyRootFilesystem"].as_bool(),
        Some(true)
    );
    assert_eq!(
        pod["volumes"][0]["persistentVolumeClaim"]["claimName"],
        "recovery-state"
    );
    let _claim = object(&objects, "PersistentVolumeClaim", "recovery-state");
    let worker = object(&objects, "Deployment", "example-worker");
    assert_eq!(worker["spec"]["strategy"]["type"], "Recreate");
    let vars: std::collections::BTreeMap<_, _> = environment(worker).into_iter().collect();
    assert_eq!(vars["PYO3_AGENT_ID"], "example-worker");
    assert_eq!(vars["RECOVERY_JOB_ID"], "example-job");
    assert_eq!(vars["RECOVERY_SUPERVISOR_AGENT_ID"], "recovery-supervisor");
}

/// Use the session-bound health executable for every generated Kubernetes probe.
#[test]
fn probes_use_the_same_session_check_for_startup_readiness_and_liveness() {
    let (_root, objects) = generate(&scenario());
    let deployment = object(&objects, "Deployment", "recovery-supervisor");
    let container = &deployment["spec"]["template"]["spec"]["containers"][0];
    for name in ["startupProbe", "readinessProbe", "livenessProbe"] {
        assert_eq!(container[name]["exec"]["command"], json!(["et-recovery-healthcheck"]));
        assert_eq!(container[name]["timeoutSeconds"], 3_i32);
    }
    assert_eq!(container["startupProbe"]["failureThreshold"], 62_i32);
}

/// Scale startup allowance with policy bounds and reject durations Kubernetes cannot represent.
#[test]
fn startup_probe_budget_scales_with_workload_settings_and_rejects_unrepresentable_durations() {
    let mut input = scenario();
    input["recovery"]["poll_seconds"] = json!(5_u64);
    input["recovery"]["command_timeout_seconds"] = json!(20_u64);
    let (_root, objects) = generate(&input);
    let deployment = object(&objects, "Deployment", "recovery-supervisor");
    assert_eq!(
        deployment["spec"]["template"]["spec"]["containers"][0]["startupProbe"]["failureThreshold"],
        85_i32
    );
    input["recovery"]["poll_seconds"] = json!(u64::MAX);
    assert!(
        generation_error(&input)
            .to_string()
            .contains("startup probe duration exceeds Kubernetes limits")
    );
}

/// Grant mutation rights only in assisted mode and leave unrelated agents unmodified.
#[test]
fn intervention_permissions_are_opt_in_and_other_agents_remain_unchanged() {
    let mut input = scenario();
    input["recovery"]["mode"] = json!("controller-assisted");
    input["recovery"]["limits"] = json!({"cpus": 1_u32, "memory": "256 MiB"});
    let mut other = input["agents"][0].clone();
    other["name"] = json!("other-worker");
    input["agents"].as_array_mut().unwrap().push(other);
    let (_root, objects) = generate(&input);
    assert_eq!(
        object(&objects, "Role", "recovery-supervisor")["rules"][0]["verbs"],
        json!(["get", "patch"])
    );
    let other = object(&objects, "Deployment", "other-worker");
    assert!(other["spec"]["strategy"].is_null());
    assert!(!environment(other).iter().any(|(key, _)| key == "PYO3_AGENT_ID"));
    let pod = &object(&objects, "Deployment", "recovery-supervisor")["spec"]["template"]["spec"];
    assert_eq!(pod["containers"][0]["resources"]["limits"]["memory"], "256Mi");
}

/// Render supervised continuous workers and unrelated scheduled runners through the same k3s generation loop.
#[test]
fn scheduled_non_supervised_runners_keep_their_cronjob_without_recovery_bindings() {
    let mut input = scenario();
    let mut auxiliary = input["agents"][0].clone();
    auxiliary["name"] = json!("auxiliary-worker");
    input["agents"].as_array_mut().unwrap().push(auxiliary);
    let mut scheduled = input["agents"][0].clone();
    scheduled["name"] = json!("scheduled-worker");
    scheduled["schedule"] = json!({"cron":"*/5 * * * *"});
    input["agents"].as_array_mut().unwrap().push(scheduled);
    let (_root, objects) = generate(&input);
    let cronjob = object(&objects, "CronJob", "scheduled-worker");
    assert_eq!(cronjob["spec"]["schedule"], "*/5 * * * *");
    let pod = &cronjob["spec"]["jobTemplate"]["spec"]["template"]["spec"];
    assert_eq!(pod["containers"][0]["name"], "scheduled-worker");
    assert_eq!(pod["containers"][0]["image"], "et-ws-pyo3-runner:latest");
    assert!(
        pod["containers"][0]["env"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| { item["name"] != "RECOVERY_SUPERVISOR_AGENT_ID" && item["name"] != "RECOVERY_JOB_ID" })
    );
    assert_eq!(
        object(&objects, "Deployment", "example-worker")["spec"]["strategy"]["type"],
        "Recreate"
    );
    assert!(
        environment(object(&objects, "Deployment", "auxiliary-worker"))
            .iter()
            .all(|(name, _value)| !name.starts_with("RECOVERY_"))
    );
    let config: EnvConfig =
        serde_env::from_iter(environment(object(&objects, "Deployment", "recovery-supervisor"))).unwrap();
    assert_eq!(config.recovery.workloads.len(), 1);
    assert_eq!(config.recovery.workloads[0].deployment, "example-worker");
}

/// A bare hub must still emit its infrastructure when the runner-generation loop has no workloads.
#[test]
fn no_runners_generate_only_hub_infrastructure_without_supervision() {
    let mut input = scenario();
    input["agents"] = json!([]);
    let _recovery = input.as_object_mut().unwrap().remove("recovery");
    let (_root, objects) = generate(&input);
    assert_no_supervisor(&objects);
    let deployments: Vec<_> = objects
        .iter()
        .filter(|object| object["kind"] == "Deployment")
        .map(object_name)
        .collect();
    assert_eq!(deployments, ["openobserve", "ws-server"]);
    assert!(!objects.iter().any(|object| object["kind"] == "CronJob"));
}

/// Validate the public runtime-settings conversion directly, including failures before manifest generation.
#[test]
fn runtime_settings_preserve_scenario_policy_and_reject_invalid_namespaces() {
    let mut settings: RecoverySettings = serde_json::from_value(scenario()["recovery"].clone()).unwrap();
    settings.mode = Mode::ControllerAssisted;
    settings.poll_seconds = 5;
    settings.command_timeout_seconds = 20;
    let config = settings.config("custom-namespace").unwrap();
    assert_eq!(config.mode, Mode::ControllerAssisted);
    assert_eq!(config.namespace, "custom-namespace");
    assert_eq!(config.job_id, settings.job_id);
    assert_eq!(config.poll_seconds, settings.poll_seconds);
    assert_eq!(config.command_timeout_seconds, settings.command_timeout_seconds);
    assert_eq!(
        serde_json::to_value(&config.policy).unwrap(),
        serde_json::to_value(&settings.policy).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&config.workloads).unwrap(),
        serde_json::to_value(&settings.workloads).unwrap()
    );
    assert!(
        settings
            .config("invalid namespace")
            .unwrap_err()
            .to_string()
            .contains("namespace")
    );
    let mut invalid = settings;
    invalid.poll_seconds = 0;
    assert!(matches!(
        invalid.config("custom-namespace"),
        Err(et_cli::CliError::RecoveryConfig(_))
    ));
}

/// Keep omitted recovery inert and use published supervisor images for released scenarios.
#[test]
fn omitted_recovery_emits_no_supervisor_and_published_recovery_uses_released_image_names() {
    let mut input = scenario();
    let _removed = input.as_object_mut().unwrap().remove("recovery");
    let (_root, objects) = generate(&input);
    assert_no_supervisor(&objects);
    let mut input = scenario();
    input["artifact_source"] = json!("published");
    let (root, objects) = generate(&input);
    assert_eq!(
        object(&objects, "Deployment", "recovery-supervisor")["spec"]["template"]["spec"]["containers"][0]["image"],
        "ghcr.io/edge-toolkit/core/et-recovery-supervisor:latest"
    );
    let readme = fs::read_to_string(root.path().join("output/README.md")).unwrap();
    assert!(readme.contains("et-recovery-supervisor:latest"));
}

/// Reject invalid policies, unsupported runner types and conflicting bindings before writing output.
#[test]
fn invalid_bindings_and_policies_fail_before_creating_output() {
    for (key, value) in [
        ("poll_seconds", json!(0_u64)),
        ("workloads", json!([])),
        (
            "workloads",
            json!([{"deployment":"outside","agent_id":"a","source":"/modules/a"}]),
        ),
        (
            "policy",
            json!({"detection_grace_seconds": 3_u32, "restart_after_seconds":20_u32,
            "stall_seconds":180_u32, "deadline_seconds":1_u32, "max_restarts":2_u32}),
        ),
    ] {
        let mut input = scenario();
        input["recovery"][key] = value;
        rejects(&input, OutputType::K3s);
    }
    for name in ["PYO3_AGENT_ID", "RECOVERY_JOB_ID", "RECOVERY_SUPERVISOR_AGENT_ID"] {
        let mut input = scenario();
        input["agents"][0]["env"] = json!({name: "conflicting"});
        rejects(&input, OutputType::K3s);
    }
    for kind in ["wasi", "web"] {
        let mut input = scenario();
        input["agents"][0]["runner"] = json!(kind);
        rejects(&input, OutputType::K3s);
    }
    let mut input = scenario();
    input["agents"][0]["schedule"] = json!({"cron":"*/5 * * * *"});
    rejects(&input, OutputType::K3s);
    let mut input = scenario();
    input["agents"][0]["name"] = json!("recovery-supervisor");
    rejects(&input, OutputType::K3s);
    let mut input = scenario();
    let duplicate = input["recovery"]["workloads"][0].clone();
    input["recovery"]["workloads"].as_array_mut().unwrap().push(duplicate);
    rejects(&input, OutputType::K3s);
    for kind in [
        OutputType::Mise,
        OutputType::DockerCompose,
        OutputType::ProcessCompose,
        OutputType::Dekit,
    ] {
        rejects(&scenario(), kind);
    }
}

/// Preserve compatible explicit identities and prepare requested container-limit support.
#[test]
fn matching_identity_settings_are_preserved_and_supervisor_limits_prepare_plugins() {
    let mut input = scenario();
    input["agents"][0]["env"] = json!({"PYO3_AGENT_ID":"example-worker"});
    input["recovery"]["limits"] = json!({"pids": 100_u64});
    let (_root, objects) = generate(&input);
    assert!(objects.iter().any(|item| item["kind"] == "DaemonSet"));
    let vars: std::collections::BTreeMap<_, _> = environment(object(&objects, "Deployment", "example-worker"))
        .into_iter()
        .collect();
    assert_eq!(vars["PYO3_AGENT_ID"], "example-worker");
}

/// Require unsupported scenario generation to fail without creating its output directory.
fn rejects(input: &Value, kind: OutputType) {
    let root = tempfile::tempdir().unwrap();
    let file = write_input(root.path(), input);
    let output = root.path().join("output");
    let _error = generate_deployment(&file, &output, Some(kind)).unwrap_err();
    assert!(!output.exists());
}

/// Preserve useful field errors and distinguish explicit null from malformed recovery settings.
#[test]
fn malformed_recovery_fields_keep_actionable_errors_and_null_remains_disabled() {
    let mut input = scenario();
    input["recovery"]["limits"] = json!({"pids_limit": 100_u64});
    assert!(generation_error(&input).to_string().contains("pids_limit"));
    input["recovery"] = Value::Null;
    let (_root, objects) = generate(&input);
    assert_no_supervisor(&objects);
    input["recovery"] = json!(true);
    assert!(
        generation_error(&input)
            .to_string()
            .contains("recovery configuration mapping")
    );
}

/// Place the supervisor on the hub machine and include its image in deployment instructions.
#[test]
fn multi_machine_instructions_include_the_supervisor_image_on_the_server() {
    let mut input = scenario();
    input["machines"] = json!([
        {"name":"server", "address":"192.168.1.10"},
        {"name":"worker", "address":"192.168.1.11", "agents":["example-worker"]}
    ]);
    let (root, objects) = generate(&input);
    let readme = fs::read_to_string(root.path().join("output/README.md")).unwrap();
    assert!(readme.contains("docker save et-recovery-supervisor:latest | ssh 192.168.1.10"));
    let supervisor = object(&objects, "Deployment", "recovery-supervisor");
    let label = format!("{}/machine", et_org::org!());
    assert_eq!(supervisor["spec"]["template"]["spec"]["nodeSelector"][&label], "server");
}

/// Bind multiple declared workers to one supervisor without implicitly enabling intervention.
#[test]
fn one_supervisor_binds_multiple_workloads_and_defaults_to_observation() {
    let mut input = scenario();
    let _mode = input["recovery"].as_object_mut().unwrap().remove("mode");
    input["agents"].as_array_mut().unwrap().push(json!({
        "name":"second-worker", "runner":"pyo3", "resources":[{"type":"pyo3-math1"}]
    }));
    input["recovery"]["workloads"].as_array_mut().unwrap().push(json!({
        "deployment":"second-worker", "agent_id":"second-client", "source":"/modules/second"
    }));
    let (_root, objects) = generate(&input);
    let role = object(&objects, "Role", "recovery-supervisor");
    assert_eq!(
        role["rules"][0]["resourceNames"],
        json!(["example-worker", "second-worker"])
    );
    let config: EnvConfig =
        serde_env::from_iter(environment(object(&objects, "Deployment", "recovery-supervisor"))).unwrap();
    assert_eq!(config.recovery.mode, Mode::ObserveOnly);
    assert_eq!(config.recovery.workloads.len(), 2);
    let vars: std::collections::BTreeMap<_, _> = environment(object(&objects, "Deployment", "second-worker"))
        .into_iter()
        .collect();
    assert_eq!(vars["PYO3_AGENT_ID"], "second-client");
}
