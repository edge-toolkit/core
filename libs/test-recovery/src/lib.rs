//! Shared inputs for recovery supervisor integration tests.
#![expect(
    clippy::unwrap_used,
    reason = "invalid test fixtures must fail the calling integration test"
)]

use chrono::Utc;
use edge_toolkit::ws::CloudEvent;
use et_recovery_supervisor::config::{Config, Mode};
use et_recovery_supervisor::policy::{Budget, Supervisor};

/// Read the repository-owned recovery example as a validated test input.
#[must_use]
pub fn config() -> Config {
    let path = edge_toolkit::config::get_project_root().join("services/recovery-supervisor/recovery.example.toml");
    toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
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
