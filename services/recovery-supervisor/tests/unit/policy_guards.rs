//! Private invariant checks remain fail-closed even when callers supply inconsistent scheduling state.
#![cfg(test)]

use super::{Budget, Decision, Health, ObservationTime, Reason, Status, Supervisor, Workload};
use crate::RecoveryError;
use crate::config::{EnvConfig, Mode};

/// Establish a real detected incident through the public observation API before testing its private guards.
fn detected_supervisor() -> (Supervisor, Workload) {
    let settings: EnvConfig = serde_env::from_iter(et_test_recovery::environment()).unwrap();
    let mut config = settings.recovery;
    config.mode = Mode::ControllerAssisted;
    let workload = config.workloads[0].clone();
    let budget = Budget::new(config.job_id.clone());
    let mut supervisor = Supervisor::new(config, budget).unwrap();
    assert_eq!(
        supervisor.observe(&workload, Health::Unready, 0).unwrap(),
        Decision::Observe
    );
    assert_eq!(
        supervisor.observe(&workload, Health::Unready, 3).unwrap(),
        Decision::Observe
    );
    (supervisor, workload)
}

/// Each reservation invariant independently rejects the action, including every non-detected incident status.
#[test]
fn reservation_recheck_rejects_disabled_mode_exhausted_budget_cooldown_and_closed_incidents() {
    for (mode, attempts, now, status) in [
        (Mode::ObserveOnly, 0, 23, Status::Detected),
        (Mode::ControllerAssisted, 2, 23, Status::Detected),
        (Mode::ControllerAssisted, 0, 22, Status::Detected),
        (Mode::ControllerAssisted, 0, 2, Status::Detected),
        (Mode::ControllerAssisted, 0, 23, Status::Suspected),
        (Mode::ControllerAssisted, 0, 23, Status::Recovered),
        (Mode::ControllerAssisted, 0, 23, Status::Transient),
        (Mode::ControllerAssisted, 0, 23, Status::Unrecovered),
    ] {
        let (mut supervisor, workload) = detected_supervisor();
        supervisor.config.mode = mode;
        supervisor.incidents[0].status = status;
        let error = Supervisor::validate_reservation(
            &supervisor.config,
            &supervisor.states[&workload.deployment],
            &supervisor.incidents[0],
            attempts,
            now,
        )
        .unwrap_err();
        assert!(matches!(error, RecoveryError::Invalid(_)));
        assert!(error.to_string().contains("restart is not eligible"));
        assert!(supervisor.budget.restarts.is_empty());
        assert_eq!(supervisor.incidents[0].actions, Vec::<super::Action>::new());
    }
}

/// The exact cooldown boundary permits reservation while the last available restart remains in budget.
#[test]
fn reservation_recheck_accepts_the_cooldown_boundary_and_remaining_budget() {
    let (supervisor, workload) = detected_supervisor();
    for attempts in [0, 1] {
        Supervisor::validate_reservation(
            &supervisor.config,
            &supervisor.states[&workload.deployment],
            &supervisor.incidents[0],
            attempts,
            23,
        )
        .unwrap();
    }
}

/// A scheduling reason cannot promise a retry time for a target absent from the configured state map.
#[test]
fn unknown_workload_has_no_promised_action_time() {
    let (supervisor, mut workload) = detected_supervisor();
    workload.deployment = "unconfigured-worker".into();
    assert_eq!(
        supervisor.next_action_time(&workload, Reason::RestartEligible),
        ObservationTime::Pending
    );
}

/// A stale cooldown assessment cannot promise an action after the workload has returned to its healthy phase.
#[test]
fn healthy_workload_has_no_promised_action_time() {
    let (supervisor, workload) = detected_supervisor();
    let healthy = Supervisor::new(supervisor.config, supervisor.budget).unwrap();
    assert_eq!(
        healthy.next_action_time(&workload, Reason::Cooldown),
        ObservationTime::Pending
    );
}

/// An inconsistent incident reference fails closed instead of reporting an action time or panicking.
#[test]
fn missing_incident_has_no_promised_action_time() {
    let (mut supervisor, workload) = detected_supervisor();
    assert_eq!(
        supervisor.next_action_time(&workload, Reason::RestartEligible),
        ObservationTime::Recorded(23)
    );
    supervisor.incidents.clear();
    assert_eq!(
        supervisor.next_action_time(&workload, Reason::RestartEligible),
        ObservationTime::Pending
    );
}

/// The post-eligibility step cannot reserve a deployment absent from the configured state map.
#[test]
fn reservation_step_rejects_unknown_workloads() {
    let (mut supervisor, mut workload) = detected_supervisor();
    workload.deployment = "unconfigured-worker".into();
    let error = supervisor.reserve_eligible_restart(&workload, 23).unwrap_err();
    assert!(error.to_string().contains("restart target is not configured"));
    assert!(supervisor.budget.restarts.is_empty());
    assert_eq!(supervisor.incidents[0].actions, Vec::<super::Action>::new());
}

/// A stale eligibility decision cannot reserve a restart after the workload returns to its healthy phase.
#[test]
fn reservation_step_rejects_workloads_without_an_incident() {
    let (supervisor, workload) = detected_supervisor();
    let mut healthy = Supervisor::new(supervisor.config, supervisor.budget).unwrap();
    let error = healthy.reserve_eligible_restart(&workload, 23).unwrap_err();
    assert!(error.to_string().contains("no incident permits this restart"));
    assert!(healthy.budget.restarts.is_empty());
    assert_eq!(healthy.last_reservation, ObservationTime::Pending);
}

/// An invalid incident reference cannot consume budget, reserve fleet capacity or move the action clock.
#[test]
fn reservation_step_rejects_missing_incident_state() {
    let (mut supervisor, workload) = detected_supervisor();
    supervisor.incidents.clear();
    let error = supervisor.reserve_eligible_restart(&workload, 23).unwrap_err();
    assert!(error.to_string().contains("incident state is absent"));
    assert_eq!(supervisor.budget.restarts[&workload.deployment], 0);
    assert!(!supervisor.states[&workload.deployment].in_flight);
    assert_eq!(supervisor.states[&workload.deployment].last_action, 3);
    assert_eq!(supervisor.last_reservation, ObservationTime::Pending);
}
