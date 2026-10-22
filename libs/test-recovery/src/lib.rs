//! Shared inputs for recovery supervisor integration tests.
#![expect(
    clippy::unwrap_used,
    reason = "invalid test fixtures must fail the calling integration test"
)]

use std::collections::BTreeMap;

use chrono::Utc;
use edge_toolkit::ws::CloudEvent;
use et_recovery_supervisor::config::{Config, EnvConfig, Mode};
use et_recovery_supervisor::policy::{Budget, Supervisor};

pub mod command;

/// Parse a deterministic test environment using the supervisor's production configuration shape.
#[must_use]
pub fn config() -> Config {
    let config: EnvConfig = serde_env::from_iter(environment()).unwrap();
    config.recovery
}

/// Build test settings without mutating the test process's environment or requiring deployment files.
#[must_use]
pub fn environment() -> BTreeMap<String, String> {
    let values = [
        ("RECOVERY_AGENT_ID", "recovery-supervisor"),
        ("RECOVERY_COMMAND_TIMEOUT_SECONDS", "10"),
        ("RECOVERY_CONTEXT", "k3d-example"),
        ("RECOVERY_JOB_ID", "example-job"),
        ("RECOVERY_MODE", "observe-only"),
        ("RECOVERY_NAMESPACE", "example"),
        ("RECOVERY_POLICY_DEADLINE_SECONDS", "240"),
        ("RECOVERY_POLICY_DETECTION_GRACE_SECONDS", "3"),
        ("RECOVERY_POLICY_MAX_RESTARTS", "2"),
        ("RECOVERY_POLICY_RESTART_AFTER_SECONDS", "20"),
        ("RECOVERY_POLICY_STALL_SECONDS", "180"),
        ("RECOVERY_POLL_SECONDS", "2"),
        ("RECOVERY_STATE_FILE", "target/recovery-state.json"),
        // Intentionally default to a loopback-only test endpoint; configuration parsing does not open a connection.
        // nosemgrep: config.semgrep.no-semgrep-mentions -- external rule annotation
        // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket
        ("RECOVERY_WS_URL", "ws://127.0.0.1:8080/ws"),
    ];
    let mut env: BTreeMap<String, String> = values
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect();
    let workloads = serde_json::json!([{
        "deployment": "example-worker", "agent_id": "example-worker", "source": "/modules/example-worker"
    }]);
    let _previous = env.insert("RECOVERY_WORKLOADS".into(), workloads.to_string());
    env
}

/// Construct an isolated recovery policy using the example workload.
#[must_use]
pub fn supervisor(mode: Mode) -> Supervisor {
    let mut config = config();
    config.mode = mode;
    Supervisor::new(config.clone(), Budget::new(config.job_id)).unwrap()
}

/// Build a lifecycle event for the example application identity and job.
#[must_use]
pub fn event(action: &str, progress: u64, sequence: u64, instance: &str) -> CloudEvent {
    CloudEvent::new(
        format!("event-{sequence}"),
        "/modules/example-worker".into(),
        format!("et.recovery.{action}"),
        Utc::now(),
        serde_json::json!({ "schema_version": 1_u64, "job_id": "example-job", "instance": instance,
            "sequence": sequence, "progress": progress }),
    )
}

/// Convert a typed report into its wire representation for protocol assertions.
#[must_use]
pub fn report(supervisor: &Supervisor) -> serde_json::Value {
    serde_json::to_value(supervisor.report()).unwrap()
}
