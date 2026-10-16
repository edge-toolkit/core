//! The production environment shape accepts explicit workload lists and rejects malformed or unsafe settings.
#![cfg(test)]

use et_recovery_supervisor::config::{Config, EnvConfig, Mode};
use et_test_recovery::environment;

/// Keep recovery opt-in while allowing unrelated service settings in the process environment.
#[test]
fn example_environment_defaults_to_observation_and_ignores_unrelated_variables() {
    let mut values = environment();
    let _mode = values.remove("RECOVERY_MODE");
    let _previous = values.insert("UNRELATED_SETTING".into(), "not recovery configuration".into());
    let parsed: EnvConfig = serde_env::from_iter(values).unwrap();
    parsed.recovery.validate().unwrap();
    assert_eq!(parsed.recovery.mode, Mode::ObserveOnly);
    assert_eq!(parsed.recovery.policy.stall_seconds, 180);
    assert_eq!(parsed.recovery.workloads.len(), 1);
    assert_eq!(parsed.recovery.workloads[0].agent_id, "example-worker");
}

/// Load multiple sender bindings and explicit intervention settings without changing process-wide state.
#[test]
fn multiple_workloads_and_assisted_mode_deserialize_without_global_environment_changes() {
    let mut values = environment();
    let _previous = values.insert("RECOVERY_MODE".into(), "controller-assisted".into());
    let _previous = values.insert(
        "RECOVERY_WORKLOADS".into(),
        serde_json::json!([
            {"deployment":"worker-1", "agent_id":"client-1", "source":"/modules/client"},
            {"deployment":"worker-2", "agent_id":"client-2", "source":"/modules/client"}
        ])
        .to_string(),
    );
    let parsed: EnvConfig = serde_env::from_iter(values).unwrap();
    parsed.recovery.validate().unwrap();
    assert_eq!(parsed.recovery.mode, Mode::ControllerAssisted);
    assert_eq!(parsed.recovery.workloads.len(), 2);
    assert_eq!(parsed.recovery.workloads[1].deployment, "worker-2");
    let encoded = serde_json::to_value(&parsed.recovery).unwrap();
    let restored: Config = serde_json::from_value(encoded).unwrap();
    assert_eq!(restored.workloads[1].agent_id, "client-2");
}

/// Reject missing settings, malformed policy values and invalid encoded workload lists.
#[test]
fn malformed_environment_values_and_missing_fields_fail_deserialization() {
    for (key, value) in [
        ("RECOVERY_WORKLOADS", "not json"),
        ("RECOVERY_WORKLOADS", "{}"),
        ("RECOVERY_WORKLOADS", "[{}]"),
        ("RECOVERY_WORKLOADS", "[1]"),
        (
            "RECOVERY_WORKLOADS",
            r#"[{"deployment":"a","agent_id":"b","source":"c","extra":1}]"#,
        ),
        ("RECOVERY_MODE", "automatic"),
        ("RECOVERY_POLL_SECONDS", "abc"),
        ("RECOVERY_POLICY_STALL_SECONDS", "-1"),
        ("RECOVERY_POLICY_MAX_RESTARTS", "4294967296"),
    ] {
        let mut values = environment();
        let _previous = values.insert(key.into(), value.into());
        let parsed: Result<EnvConfig, _> = serde_env::from_iter(values);
        assert!(parsed.is_err(), "{key}: {value}");
    }
    for key in ["RECOVERY_JOB_ID", "RECOVERY_WORKLOADS", "RECOVERY_POLICY_STALL_SECONDS"] {
        let mut values = environment();
        let _removed = values.remove(key);
        let parsed: Result<EnvConfig, _> = serde_env::from_iter(values);
        assert!(parsed.is_err(), "{key}");
    }
    let mut invalid = serde_json::to_value(et_test_recovery::config()).unwrap();
    invalid["workloads"] = serde_json::json!(1_u64);
    let _error = serde_json::from_value::<Config>(invalid).unwrap_err();
}

/// Reject configuration that parses successfully but cannot support safe recovery.
#[test]
fn parsed_environment_still_requires_semantic_validation() {
    for (key, value) in [
        ("RECOVERY_WORKLOADS", "[]"),
        ("RECOVERY_CONTEXT", "-unsafe"),
        ("RECOVERY_POLICY_DEADLINE_SECONDS", "1"),
        ("RECOVERY_POLL_SECONDS", "0"),
    ] {
        let mut values = environment();
        let _previous = values.insert(key.into(), value.into());
        let parsed: EnvConfig = serde_env::from_iter(values).unwrap();
        assert!(parsed.recovery.validate().is_err(), "{key}");
    }
}

/// Fail startup before writing state when environment configuration is incomplete.
#[test]
fn binary_rejects_malformed_environment_before_creating_state() {
    let directory = tempfile::tempdir().unwrap();
    let state = directory.path().join("state.json");
    for (key, value) in [
        ("RECOVERY_WORKLOADS", "not json"),
        ("RECOVERY_POLL_SECONDS", "0"),
        ("RECOVERY_MODE", "automatic"),
    ] {
        // Raw output is intentional: rejection before any external action is the outcome under test.
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_et-recovery-supervisor"))
            .envs(environment())
            .env("RECOVERY_STATE_FILE", &state)
            .env(key, value)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{key}");
        assert!(!output.stderr.is_empty(), "{key}");
        assert!(!state.exists(), "{key}");
    }
}
