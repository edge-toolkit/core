//! Configuration and deployment observation reject ambiguous or unsafe targets before any action.
#![cfg(test)]

use et_recovery_supervisor::config::{Mode, valid_name};
use et_recovery_supervisor::kubernetes::{Kubernetes, deployment_health};
use et_recovery_supervisor::policy::{Budget, Health, Supervisor};
use et_test_recovery::{config, event, supervisor};

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
        let mut value = toml::Value::try_from(config_for_serialization()).unwrap();
        value[field] = toml::Value::String(String::default());
        let invalid: et_recovery_supervisor::config::Config = value.try_into().unwrap();
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

fn config_for_serialization() -> serde_json::Value {
    serde_json::to_value(config()).unwrap()
}

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

#[test]
fn readiness_requires_current_generation_and_one_available_updated_replica() {
    let path = edge_toolkit::config::get_project_root()
        .join("services/recovery-supervisor/tests/fixtures/deployment-ready.yaml");
    let mut value: serde_json::Value = serde_yaml::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(
        deployment_health(&serde_json::to_vec(&value).unwrap()).unwrap(),
        Health::Ready
    );
    value["status"]["observedGeneration"] = serde_json::json!(1_u64);
    assert_eq!(
        deployment_health(&serde_json::to_vec(&value).unwrap()).unwrap(),
        Health::Unready
    );
    let _status = value.as_object_mut().unwrap().remove("status");
    assert_eq!(
        deployment_health(&serde_json::to_vec(&value).unwrap()).unwrap(),
        Health::Unready
    );
    value["spec"]["replicas"] = serde_json::json!(2_u64);
    let _error = deployment_health(&serde_json::to_vec(&value).unwrap()).unwrap_err();
    let _error = deployment_health(b"invalid json").unwrap_err();
}
