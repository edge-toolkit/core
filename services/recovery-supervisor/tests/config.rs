//! Configuration and deployment observation reject ambiguous or unsafe targets before any action.
#![cfg(test)]

use et_recovery_supervisor::config::{Config, Mode, valid_name};
use et_recovery_supervisor::kubernetes::{Kubernetes, deployment_health};
use et_recovery_supervisor::policy::{Budget, Health, Supervisor};
use et_test_recovery::{config, event, supervisor};
use fs_err as fs;

/// Typed construction preserves the complete in-cluster configuration without encoding it through JSON.
#[test]
fn typed_in_cluster_settings_match_the_expected_runtime_configuration() {
    let mut expected = config();
    expected.context = "in-cluster".into();
    expected.state_file = "/app/state/recovery.json".into();
    let actual = Config::in_cluster(
        expected.ws_url.clone(),
        expected.namespace.clone(),
        expected.job_id.clone(),
        expected.policy.clone(),
        expected.workloads.clone(),
    );
    actual.validate().unwrap();
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
}

/// Validate the default fixture while rejecting mismatched restored job budgets.
#[test]
fn example_is_valid_and_observe_only_by_default() {
    let config = config();
    config.validate().unwrap();
    let _backend = Kubernetes::new(config.clone()).unwrap();
    assert_eq!(config.mode, Mode::ObserveOnly);
    let mut policy = supervisor(Mode::ObserveOnly);
    policy
        .event("example-worker", &event("started", 0, 1, "one"), 0)
        .unwrap();
    let mut budget = Budget::new("wrong-job".into());
    let _error = Supervisor::new(config.clone(), budget.clone()).unwrap_err();
    budget.job_id.clone_from(&config.job_id);
    budget.schema_version = 2;
    let _error = Supervisor::new(config, budget).unwrap_err();
}

/// Reject unsafe deployment targets, missing identities and duplicate workload bindings.
#[test]
fn targets_and_identities_are_explicit_and_unique() {
    for name in [
        "",
        "-flag",
        "worker/../other",
        "worker/other",
        "UPPER",
        "worker-",
        "label=value",
    ] {
        assert!(!valid_name(name));
    }
    assert!(!valid_name(&"a".repeat(64)));
    assert!(valid_name("worker-1"));
    for field in ["context", "namespace", "agent_id", "job_id", "state_file", "ws_url"] {
        let mut value = config_for_serialization();
        value[field] = serde_json::json!("");
        let invalid: et_recovery_supervisor::config::Config = serde_json::from_value(value).unwrap();
        assert!(invalid.validate().is_err(), "{field}");
    }
    let mut duplicate = config();
    duplicate.workloads.push(duplicate.workloads[0].clone());
    let _error = duplicate.validate().unwrap_err();
    duplicate.workloads.clear();
    let _error = duplicate.validate().unwrap_err();
    for field in ["deployment", "agent_id", "source"] {
        let mut value = config_for_serialization();
        value["workloads"][0][field] = serde_json::json!("");
        let invalid = serde_json::from_value::<et_recovery_supervisor::config::Config>(value).unwrap();
        assert!(invalid.validate().is_err(), "{field}");
    }
}

/// Expose a default configuration as JSON for focused invalid-field mutations.
fn config_for_serialization() -> serde_json::Value {
    serde_json::to_value(config()).unwrap()
}

/// Reject malformed connection URLs without forbidding trusted internal plaintext deployments.
#[test]
fn websocket_endpoints_require_a_valid_host_and_no_embedded_credentials() {
    // These malformed URLs are parser inputs only; no network connections are established.
    for endpoint in [
        "not a url",
        "https://127.0.0.1/ws",
        "wss://",
        "wss://user@hub/ws",
        "wss://user:secret@hub/ws",
        "wss://hub/ws#fragment",
    ] {
        let mut value = config();
        value.ws_url = endpoint.into();
        assert!(value.validate().is_err(), "{endpoint}");
    }
    for endpoint in [
        "wss://hub.example/ws",
        // Intentionally validate plaintext compatibility for internal deployments; this input never opens a
        // connection.
        // nosemgrep: config.semgrep.no-semgrep-mentions -- external rule annotation
        // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket
        "ws://ws-server:8080/ws",
    ] {
        let mut value = config();
        value.ws_url = endpoint.into();
        value.validate().unwrap();
    }
}

/// Reject disabled timing bounds and policies that leave no time for recovery.
#[test]
fn zero_timings_and_impossible_recovery_deadlines_are_rejected() {
    for field in [
        "detection_grace_seconds",
        "restart_after_seconds",
        "stall_seconds",
        "deadline_seconds",
    ] {
        let mut value = config_for_serialization();
        value["policy"][field] = serde_json::json!(0_u64);
        assert!(
            serde_json::from_value::<et_recovery_supervisor::config::Config>(value)
                .unwrap()
                .validate()
                .is_err()
        );
    }
    let mut invalid = config();
    invalid.poll_seconds = 0;
    let _error = Kubernetes::new(invalid).unwrap_err();
    let mut invalid = config();
    invalid.policy.deadline_seconds = 2;
    let _error = invalid.validate().unwrap_err();
}

/// Require a current single-replica deployment rather than stale or incomplete readiness status.
#[test]
fn readiness_requires_current_generation_and_one_available_updated_replica() {
    let path = edge_toolkit::config::get_project_root()
        .join("services/recovery-supervisor/tests/fixtures/deployment-ready.yaml");
    let mut value: serde_json::Value = serde_yaml::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(
        deployment_health(&serde_json::to_vec(&value).unwrap()).unwrap(),
        Health::Ready
    );
    for (field, count) in [
        ("replicas", 2_u64),
        ("unavailableReplicas", 1),
        ("observedGeneration", 1),
        ("availableReplicas", 0),
        ("updatedReplicas", 0),
    ] {
        let mut unready = value.clone();
        unready["status"][field] = serde_json::json!(count);
        assert_eq!(
            deployment_health(&serde_json::to_vec(&unready).unwrap()).unwrap(),
            Health::Unready,
            "{field}={count} must prevent readiness"
        );
    }
    let _status = value.as_object_mut().unwrap().remove("status");
    assert_eq!(
        deployment_health(&serde_json::to_vec(&value).unwrap()).unwrap(),
        Health::Unready
    );
    value["spec"]["replicas"] = serde_json::json!(2_u64);
    let _error = deployment_health(&serde_json::to_vec(&value).unwrap()).unwrap_err();
    let _error = deployment_health(b"invalid json").unwrap_err();
}
