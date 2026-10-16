//! Recovery policy verifies useful work, enforces action bounds, and rejects stale or foreign evidence.
#![cfg(test)]

use et_recovery_supervisor::config::Mode;
use et_recovery_supervisor::policy::{Action, Decision, Health, ObservationTime, Status, Supervisor};
use et_test_recovery::{config, event, report, supervisor};

#[test]
fn readiness_does_not_establish_recovery_but_progress_does() {
    let mut supervisor = supervisor(Mode::ObserveOnly);
    let workload = config().workloads.remove(0);
    supervisor
        .event("example-worker", &event("started", 0, 1, "first"), 0)
        .unwrap();
    supervisor
        .event("example-worker", &event("progress", 1, 2, "first"), 1)
        .unwrap();
    assert_eq!(
        supervisor.observe(&workload, Health::Unready, 10).unwrap(),
        Decision::Observe
    );
    assert_eq!(
        supervisor.observe(&workload, Health::Unready, 13).unwrap(),
        Decision::Observe
    );
    assert_eq!(supervisor.incidents()[0].status, Status::Detected);
    assert_eq!(supervisor.incidents()[0].detected, ObservationTime::Recorded(13));
    let _decision = supervisor.observe(&workload, Health::Ready, 14).unwrap();
    assert_eq!(supervisor.incidents()[0].status, Status::Detected);
    supervisor
        .event("example-worker", &event("started", 1, 1, "replacement"), 15)
        .unwrap();
    let _decision = supervisor.observe(&workload, Health::Ready, 16).unwrap();
    assert_eq!(supervisor.incidents()[0].status, Status::Detected);
    supervisor
        .event("example-worker", &event("checkpoint-restored", 1, 2, "replacement"), 16)
        .unwrap();
    let _decision = supervisor.observe(&workload, Health::Ready, 17).unwrap();
    assert_eq!(supervisor.incidents()[0].status, Status::Detected);
    supervisor
        .event("example-worker", &event("progress", 2, 3, "replacement"), 18)
        .unwrap();
    let _decision = supervisor.observe(&workload, Health::Ready, 19).unwrap();
    assert_eq!(supervisor.incidents()[0].status, Status::Recovered);
    assert_eq!(report(&supervisor)["recovery_seconds"], serde_json::json!([9_u64]));
    assert_eq!(report(&supervisor)["mttr_seconds"].as_f64(), Some(9.0_f64));
}

#[test]
fn unknown_observations_do_not_trigger_actions_or_fake_recovery() {
    let mut supervisor = supervisor(Mode::ControllerAssisted);
    let workload = config().workloads.remove(0);
    assert_eq!(
        supervisor.observe(&workload, Health::Unknown, 0).unwrap(),
        Decision::Observe
    );
    assert!(supervisor.incidents().is_empty());
    let _decision = supervisor.observe(&workload, Health::Unready, 1).unwrap();
    let _decision = supervisor.observe(&workload, Health::Unready, 4).unwrap();
    assert_eq!(
        supervisor.observe(&workload, Health::Unknown, 100).unwrap(),
        Decision::Observe
    );
    assert_eq!(supervisor.report().unknown_observations, 2_u64);
    assert_eq!(supervisor.incidents()[0].status, Status::Detected);
}

#[test]
fn initial_committed_progress_becomes_the_failure_baseline() {
    let mut policy = supervisor(Mode::ObserveOnly);
    let worker = config().workloads.remove(0);
    policy
        .event("example-worker", &event("started", 7, 1, "restored"), 0)
        .unwrap();
    let _first = policy.observe(&worker, Health::Unready, 10).unwrap();
    let _detected = policy.observe(&worker, Health::Unready, 13).unwrap();
    policy
        .event("example-worker", &event("progress", 7, 2, "restored"), 14)
        .unwrap();
    let _unchanged = policy.observe(&worker, Health::Ready, 15).unwrap();
    assert_eq!(policy.incidents()[0].baseline, 7);
    assert_eq!(policy.incidents()[0].status, Status::Detected);
    policy
        .event("example-worker", &event("progress", 8, 3, "restored"), 16)
        .unwrap();
    let _resumed = policy.observe(&worker, Health::Ready, 17).unwrap();
    assert_eq!(policy.incidents()[0].status, Status::Recovered);
}

#[test]
fn assisted_restarts_are_delayed_bounded_and_survive_supervisor_replacement() {
    let mut supervisor = supervisor(Mode::ControllerAssisted);
    let workload = config().workloads.remove(0);
    let _error = supervisor.reserve_restart(&workload, 0).unwrap_err();
    let _decision = supervisor.observe(&workload, Health::Unready, 0).unwrap();
    let _decision = supervisor.observe(&workload, Health::Unready, 3).unwrap();
    let _error = supervisor.reserve_restart(&workload, 4).unwrap_err();
    assert_eq!(
        supervisor.observe(&workload, Health::Unready, 22).unwrap(),
        Decision::Observe
    );
    for now in [23, 43] {
        assert_eq!(
            supervisor.observe(&workload, Health::Unready, now).unwrap(),
            Decision::Restart
        );
        supervisor.reserve_restart(&workload, now).unwrap();
        supervisor.action_result(&workload, Action::RestartRequested).unwrap();
    }
    assert_eq!(
        supervisor.observe(&workload, Health::Unready, 63).unwrap(),
        Decision::Observe
    );
    let _error = supervisor.reserve_restart(&workload, 63).unwrap_err();
    let mut restored_config = config();
    restored_config.mode = Mode::ControllerAssisted;
    let mut restored = Supervisor::new(restored_config, supervisor.budget().clone()).unwrap();
    let _decision = restored.observe(&workload, Health::Unready, 0).unwrap();
    let _decision = restored.observe(&workload, Health::Unready, 3).unwrap();
    assert_eq!(
        restored.observe(&workload, Health::Unready, 23).unwrap(),
        Decision::Observe
    );
}

#[test]
fn stalled_work_can_fail_without_a_failed_container_and_completed_work_does_not_stall() {
    let mut supervisor = supervisor(Mode::ObserveOnly);
    let workload = config().workloads.remove(0);
    let _decision = supervisor.observe(&workload, Health::Ready, 300).unwrap();
    assert!(supervisor.incidents().is_empty());
    supervisor
        .event("example-worker", &event("started", 0, 1, "one"), 300)
        .unwrap();
    let _decision = supervisor.observe(&workload, Health::Ready, 480).unwrap();
    let _decision = supervisor.observe(&workload, Health::Ready, 483).unwrap();
    assert_eq!(supervisor.incidents()[0].status, Status::Detected);
    supervisor
        .event("example-worker", &event("completed", 3, 2, "one"), 490)
        .unwrap();
    let _decision = supervisor.observe(&workload, Health::Ready, 491).unwrap();
    let _decision = supervisor.observe(&workload, Health::Ready, 1000).unwrap();
    assert_eq!(supervisor.incidents().len(), 1);
}

#[test]
fn expired_recovery_is_not_a_zero_duration_success() {
    let mut supervisor = supervisor(Mode::ControllerAssisted);
    let workload = config().workloads.remove(0);
    let _decision = supervisor.observe(&workload, Health::Unready, 0).unwrap();
    let _decision = supervisor.observe(&workload, Health::Unready, 3).unwrap();
    assert_eq!(
        supervisor.observe(&workload, Health::Unready, 240).unwrap(),
        Decision::Observe
    );
    assert_eq!(supervisor.incidents()[0].status, Status::Unrecovered);
    let _error = supervisor.reserve_restart(&workload, 260).unwrap_err();
    assert_eq!(supervisor.report().recovered_incidents, 0);
    assert_eq!(report(&supervisor)["mttr_seconds"], serde_json::Value::Null);
    assert_eq!(supervisor.report().recovery_seconds, Vec::<u64>::new());
}

#[test]
fn foreign_replayed_and_out_of_order_events_cannot_override_current_evidence() {
    let mut supervisor = supervisor(Mode::ObserveOnly);
    let start = event("started", 0, 1, "one");
    let _error = supervisor.event("foreign-worker", &start, 0).unwrap_err();
    for (field, value) in [
        ("job_id", serde_json::json!("foreign")),
        ("schema_version", serde_json::json!(2_u64)),
        ("instance", serde_json::json!("")),
        ("sequence", serde_json::json!(0_u64)),
    ] {
        let mut invalid = start.clone();
        invalid.data[field] = value;
        let _error = supervisor.event("example-worker", &invalid, 0).unwrap_err();
    }
    for kind in ["other", "et.recovery.unknown"] {
        let mut invalid = start.clone();
        invalid.event_type = kind.into();
        let _error = supervisor.event("example-worker", &invalid, 0).unwrap_err();
    }
    supervisor.event("example-worker", &start, 0).unwrap();
    let _error = supervisor
        .event("example-worker", &event("progress", 1, 2, "unannounced"), 0)
        .unwrap_err();
    let _error = supervisor.event("example-worker", &start, 1).unwrap_err();
    supervisor
        .event("example-worker", &event("started", 0, 1, "two"), 1)
        .unwrap();
    assert!(
        supervisor
            .event("example-worker", &event("progress", 50, 2, "one"), 2)
            .is_err()
    );
    supervisor
        .event("example-worker", &event("failed", 0, 2, "two"), 2)
        .unwrap();
    let workload = config().workloads.remove(0);
    let _decision = supervisor.observe(&workload, Health::Ready, 2).unwrap();
    let _decision = supervisor.observe(&workload, Health::Ready, 5).unwrap();
    assert_eq!(supervisor.incidents()[0].status, Status::Detected);
}

#[test]
fn transient_unreadiness_below_grace_does_not_trigger_recovery() {
    let mut supervisor = supervisor(Mode::ObserveOnly);
    let mut workload = config().workloads.remove(0);
    let _decision = supervisor.observe(&workload, Health::Unready, 0).unwrap();
    let _decision = supervisor.observe(&workload, Health::Ready, 1).unwrap();
    assert_eq!(supervisor.incidents()[0].status, Status::Transient);
    let _error = supervisor
        .action_result(&workload, Action::RestartRequested)
        .unwrap_err();
    workload.deployment = "not-allowlisted".into();
    let _error = supervisor.observe(&workload, Health::Ready, 2).unwrap_err();
    let _error = supervisor.reserve_restart(&workload, 2).unwrap_err();
    let _error = supervisor
        .action_result(&workload, Action::RestartRequested)
        .unwrap_err();
}

#[test]
fn adopting_a_running_instance_does_not_count_old_work_as_recovery() {
    let mut observer = supervisor(Mode::ObserveOnly);
    let target = config().workloads.remove(0);
    let _suspected = observer.observe(&target, Health::Unready, 0).unwrap();
    let _detected = observer.observe(&target, Health::Unready, 3).unwrap();
    observer
        .event("example-worker", &event("progress", 7, 100, "existing"), 4)
        .unwrap();
    let _ready = observer.observe(&target, Health::Ready, 5).unwrap();
    assert_eq!(observer.incidents()[0].baseline, 7);
    assert_eq!(observer.incidents()[0].status, Status::Detected);
    observer
        .event("example-worker", &event("progress", 8, 101, "existing"), 6)
        .unwrap();
    let _recovered = observer.observe(&target, Health::Ready, 7).unwrap();
    assert_eq!(observer.incidents()[0].status, Status::Recovered);
}

#[test]
fn report_preserves_typed_checkpoint_measurements_and_distinguishes_pending_times() {
    use et_recovery_supervisor::policy::CheckpointKind;

    let mut policy = supervisor(Mode::ObserveOnly);
    let target = config().workloads.remove(0);
    policy
        .event("example-worker", &event("started", 0, 1, "one"), 0)
        .unwrap();
    let _decision = policy.observe(&target, Health::Unready, 0).unwrap();
    assert_eq!(policy.incidents()[0].detected, ObservationTime::Pending);
    assert_eq!(policy.incidents()[0].recovered, ObservationTime::Pending);
    assert_eq!(report(&policy)["incidents"][0]["detected"], serde_json::Value::Null);
    for (sequence, kind, expected) in [
        (2, "checkpoint-saved", CheckpointKind::Saved),
        (3, "checkpoint-restored", CheckpointKind::Restored),
        (4, "resume-completed", CheckpointKind::ResumeCompleted),
    ] {
        let mut measurement = event(kind, 0, sequence, "one");
        measurement.data["bytes_count"] = serde_json::json!(1024_u64);
        measurement.data["duration_millis"] = serde_json::json!(12_u64);
        policy.event("example-worker", &measurement, sequence).unwrap();
        let snapshot = policy.report();
        let latest = snapshot.checkpoint_measurements.last().unwrap();
        assert_eq!(latest.kind, expected);
        assert_eq!(latest.bytes, 1024);
        assert_eq!(latest.duration_millis, 12);
        assert_eq!(
            report(&policy)["checkpoint_measurements"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()["kind"],
            kind
        );
    }
    for time in [
        ObservationTime::Pending,
        ObservationTime::Recorded(0),
        ObservationTime::Recorded(7),
    ] {
        let serialized = serde_json::to_value(time).unwrap();
        assert_eq!(serde_json::from_value::<ObservationTime>(serialized).unwrap(), time);
    }
}
