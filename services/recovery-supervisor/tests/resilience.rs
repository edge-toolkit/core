//! Recovery coordination, durable context and stability preserve bounded actions and honest measurements.
#![cfg(test)]

use et_recovery_supervisor::config::{Config, Mode, Workload};
use et_recovery_supervisor::policy::{Action, Budget, Decision, Health, ObservationTime, Status, Supervisor};
use et_recovery_supervisor::resilience::{Classification, Reason, Stability};
use et_test_recovery::{config, event, report};

/// Create an active workload whose observed deployment failure has satisfied detection grace.
fn detected_policy(settings: Config) -> (Supervisor, Workload) {
    let worker = settings.workloads[0].clone();
    let budget = Budget::new(settings.job_id.clone());
    let mut policy = Supervisor::new(settings, budget).unwrap();
    policy
        .event(&worker.agent_id, &event("started", 0, 1, "one"), 0)
        .unwrap();
    observe_unready_at(&mut policy, &worker, &[0, 3]);
    (policy, worker)
}

/// Apply a deterministic sequence of deployment-health observations without changing lifecycle evidence.
fn observe_unready_at(policy: &mut Supervisor, worker: &Workload, times: &[u64]) {
    for &now in times {
        let _observed = policy.observe(worker, Health::Unready, now).unwrap();
    }
}

/// Configure two independently supervised deployments for fleet-admission tests.
fn two_worker_policy(mut settings: Config) -> (Supervisor, Workload, Workload) {
    let first = settings.workloads[0].clone();
    let mut second = first.clone();
    second.deployment = "second".into();
    second.agent_id = "second".into();
    settings.workloads.push(second.clone());
    let budget = Budget::new(settings.job_id.clone());
    (Supervisor::new(settings, budget).unwrap(), first, second)
}

/// Retry schedules respect both cooldowns and fleet spacing, and promise no action at or beyond the deadline.
#[test]
fn retry_schedule_is_pending_when_fleet_spacing_exhausts_the_incident_deadline() {
    for (interval, expected) in [
        (0, ObservationTime::Recorded(43)),
        (30, ObservationTime::Recorded(53)),
        (217, ObservationTime::Pending),
        (300, ObservationTime::Pending),
    ] {
        let mut settings = config();
        settings.mode = Mode::ControllerAssisted;
        settings.resilience.action_interval_seconds = interval;
        let (mut policy, worker) = detected_policy(settings);
        policy.reserve_restart(&worker, 23).unwrap();
        let _waiting = policy.observe(&worker, Health::Unready, 24).unwrap();
        let assessment = &policy.report().assessments[&worker.deployment];
        assert_eq!(assessment.reason, Reason::RecoveryInProgress);
        assert_eq!(assessment.next_action_at, expected);
        assert_eq!(policy.budget().restarts[&worker.deployment], 1);
    }
}

/// Restore one useful work unit, leaving the separate stability assessment pending.
fn recovered_policy(settings: Config) -> (Supervisor, Workload) {
    let (mut policy, worker) = detected_policy(settings);
    policy
        .event(&worker.agent_id, &event("progress", 1, 2, "one"), 4)
        .unwrap();
    let _recovered = policy.observe(&worker, Health::Ready, 5).unwrap();
    (policy, worker)
}

/// Dependency failure and missing observations suppress actions without resetting the incident deadline.
#[test]
fn dependencies_gate_restarts_and_are_reassessed_without_budget_consumption() {
    let mut settings = config();
    settings.mode = Mode::ControllerAssisted;
    let worker = settings.workloads[0].clone();
    let _previous = settings
        .resilience
        .dependencies
        .insert(worker.deployment.clone(), vec!["storage".into()]);
    let mut policy = Supervisor::new(settings, Budget::new("example-job".into())).unwrap();
    let _suspected = policy.observe(&worker, Health::Unready, 0).unwrap();
    assert_eq!(policy.observe(&worker, Health::Unready, 23).unwrap(), Decision::Observe);
    assert_eq!(
        report(&policy)["assessments"][&worker.deployment]["reason"],
        "dependency-unknown"
    );
    policy.dependency_observation("storage", Health::Unready).unwrap();
    assert_eq!(policy.observe(&worker, Health::Unready, 43).unwrap(), Decision::Observe);
    assert_eq!(
        policy.report().assessments[&worker.deployment].classification,
        Classification::DependencyUnavailable
    );
    policy.dependency_observation("storage", Health::Ready).unwrap();
    assert_eq!(policy.observe(&worker, Health::Unready, 44).unwrap(), Decision::Restart);
    assert!(policy.budget().restarts.is_empty());
    assert!(policy.dependency_observation("foreign", Health::Ready).is_err());
    assert_eq!(
        policy.report().assessments[&worker.deployment].next_action_at,
        ObservationTime::Recorded(43)
    );
    let _expired = policy.observe(&worker, Health::Unready, 240).unwrap();
    assert_eq!(policy.incidents()[0].status, Status::Unrecovered);
}

/// Build a two-level dependency graph with a supervised intermediate deployment and an external storage service.
fn transitive_policy() -> (Supervisor, Workload, Workload) {
    let mut settings = config();
    settings.mode = Mode::ControllerAssisted;
    let mut dependency = settings.workloads[0].clone();
    dependency.deployment = "dependency".into();
    dependency.agent_id = "dependency".into();
    settings.workloads.push(dependency.clone());
    settings.resilience.dependencies = [
        ("example-worker".into(), vec!["dependency".into()]),
        ("dependency".into(), vec!["storage".into()]),
    ]
    .into();
    let (policy, worker) = detected_policy(settings);
    (policy, worker, dependency)
}

/// Ready intermediate deployments cannot hide unavailable or unknown dependencies further down the graph.
#[test]
fn transitive_dependencies_block_reservations_until_the_entire_path_is_healthy() {
    let (mut policy, worker, dependency) = transitive_policy();
    policy
        .dependency_observation(&dependency.deployment, Health::Ready)
        .unwrap();
    let _unknown = policy.observe(&worker, Health::Unready, 23).unwrap();
    assert_eq!(
        policy.report().assessments[&worker.deployment].reason,
        Reason::DependencyUnknown
    );
    for (health, now, expected) in [
        (Health::Unready, 24, Reason::DependencyUnavailable),
        (Health::Unknown, 25, Reason::DependencyUnknown),
        (Health::Ready, 26, Reason::RestartEligible),
    ] {
        policy.dependency_observation("storage", health).unwrap();
        let _decision = policy.observe(&worker, Health::Unready, now).unwrap();
        let assessment = &policy.report().assessments[&worker.deployment];
        assert_eq!(assessment.reason, expected);
        if expected != Reason::RestartEligible {
            assert_eq!(assessment.next_action_at, ObservationTime::Pending);
            let _blocked = policy.reserve_restart(&worker, now).unwrap_err();
            assert!(policy.budget().restarts.is_empty());
            assert_eq!(policy.incidents()[0].actions, Vec::<Action>::default());
        }
    }
    policy.reserve_restart(&worker, 26).unwrap();
    assert_eq!(policy.budget().restarts[&worker.deployment], 1);
}

/// Deployment readiness does not make a supervised dependency healthy when its application fails or stalls.
#[test]
fn failed_or_stalled_dependencies_block_restarts_until_useful_work_resumes() {
    for (failed, now) in [(true, 24), (false, 180)] {
        let (mut policy, worker, dependency) = transitive_policy();
        policy
            .dependency_observation(&dependency.deployment, Health::Ready)
            .unwrap();
        policy.dependency_observation("storage", Health::Ready).unwrap();
        policy
            .event(&dependency.agent_id, &event("started", 0, 1, "dependency-instance"), 0)
            .unwrap();
        if failed {
            policy
                .event(&dependency.agent_id, &event("failed", 0, 2, "dependency-instance"), now)
                .unwrap();
        }
        assert_eq!(
            policy.observe(&worker, Health::Unready, now).unwrap(),
            Decision::Observe
        );
        assert_eq!(
            policy.report().assessments[&worker.deployment].reason,
            Reason::DependencyUnavailable
        );
        let _blocked = policy.reserve_restart(&worker, now).unwrap_err();
        assert!(policy.budget().restarts.is_empty());
        policy
            .event(
                &dependency.agent_id,
                &event("progress", 1, 3, "dependency-instance"),
                now + 1,
            )
            .unwrap();
        assert_eq!(
            policy.observe(&worker, Health::Unready, now + 1).unwrap(),
            Decision::Restart
        );
        policy.reserve_restart(&worker, now + 1).unwrap();
        assert_eq!(policy.budget().restarts[&worker.deployment], 1);
    }
}

/// Reservations serialize fleet admission even when several workloads were eligible in the same observation cycle.
#[test]
fn concurrent_recovery_and_action_interval_limits_apply_at_reservation() {
    let mut settings = config();
    settings.mode = Mode::ControllerAssisted;
    settings.resilience.max_concurrent_recoveries = 1;
    settings.resilience.action_interval_seconds = 30;
    let (mut policy, first, second) = two_worker_policy(settings);
    for worker in [&first, &second] {
        let _suspected = policy.observe(worker, Health::Unready, 0).unwrap();
        let _detected = policy.observe(worker, Health::Unready, 3).unwrap();
        assert_eq!(policy.observe(worker, Health::Unready, 23).unwrap(), Decision::Restart);
    }
    policy.reserve_restart(&first, 23).unwrap();
    policy.action_result(&first, Action::RestartRequested).unwrap();
    assert!(policy.reserve_restart(&second, 23).is_err());
    let _limited = policy.observe(&second, Health::Unready, 24).unwrap();
    assert_eq!(policy.report().assessments["second"].reason, Reason::ConcurrencyLimit);
    policy
        .event(&first.agent_id, &event("started", 0, 1, "one"), 24)
        .unwrap();
    policy
        .event(&first.agent_id, &event("progress", 1, 2, "one"), 25)
        .unwrap();
    let _recovered = policy.observe(&first, Health::Ready, 25).unwrap();
    let _delayed = policy.observe(&second, Health::Unready, 26).unwrap();
    assert_eq!(policy.report().assessments["second"].reason, Reason::ActionInterval);
    assert_eq!(policy.observe(&second, Health::Unready, 53).unwrap(), Decision::Restart);
    policy.reserve_restart(&second, 53).unwrap();
}

/// An expired recovery on an unpolled workload must not retain the fleet's only admission slot.
#[test]
fn expired_in_flight_recovery_does_not_block_a_later_incident() {
    let mut settings = config();
    settings.mode = Mode::ControllerAssisted;
    settings.resilience.max_concurrent_recoveries = 1;
    let (mut policy, first, second) = two_worker_policy(settings);
    observe_unready_at(&mut policy, &first, &[0, 3]);
    policy.reserve_restart(&first, 23).unwrap();
    for now in [200, 203, 223] {
        let _observed = policy.observe(&second, Health::Unready, now).unwrap();
    }
    assert_eq!(policy.report().assessments["second"].reason, Reason::ConcurrencyLimit);
    assert_eq!(
        policy.observe(&second, Health::Unready, 240).unwrap(),
        Decision::Restart
    );
    assert_eq!(policy.incidents()[0].status, Status::Detected);
    policy.reserve_restart(&second, 240).unwrap();
    assert_eq!(policy.budget().restarts["second"], 1);
}

/// A terminal incident in an external report cannot consume capacity through its stale reservation marker.
#[test]
fn restored_terminal_incident_with_a_stale_reservation_does_not_block_the_fleet() {
    let mut settings = config();
    settings.mode = Mode::ControllerAssisted;
    settings.resilience.max_concurrent_recoveries = 1;
    let deadline = settings.policy.deadline_seconds;
    let (mut policy, first, second) = two_worker_policy(settings.clone());
    settings.workloads.push(second.clone());
    observe_unready_at(&mut policy, &first, &[0, 3]);
    policy.reserve_restart(&first, 23).unwrap();
    for now in [24, 27] {
        let _observed = policy.observe(&second, Health::Unready, now).unwrap();
    }
    let mut saved = policy.report();
    saved.incidents[0].status = Status::Unrecovered;
    let mut restored = Supervisor::restore(settings, serde_json::to_value(saved).unwrap()).unwrap();
    let now = restored.elapsed(20);
    assert!(now < deadline, "the status guard must act independently of expiry");
    assert_eq!(
        restored.observe(&second, Health::Unready, now).unwrap(),
        Decision::Restart
    );
    restored.reserve_restart(&second, now).unwrap();
    assert_eq!(restored.incidents()[0].status, Status::Unrecovered);
    assert_eq!(restored.budget().restarts[&first.deployment], 1);
    assert_eq!(restored.budget().restarts[&second.deployment], 1);
}

/// The action boundary enforces the deadline even before the next observation can mark an incident unrecovered.
#[test]
fn restart_reservation_cannot_outlive_an_unpolled_incident_deadline() {
    let mut settings = config();
    settings.mode = Mode::ControllerAssisted;
    let deadline = settings.policy.deadline_seconds;
    let (mut policy, worker) = detected_policy(settings);
    assert_eq!(policy.incidents()[0].status, Status::Detected);
    let error = policy.reserve_restart(&worker, deadline).unwrap_err();
    assert!(error.to_string().contains("not eligible"));
    assert!(policy.budget().restarts.is_empty());
    assert_eq!(policy.incidents()[0].actions, Vec::<Action>::default());
    let _expired = policy.observe(&worker, Health::Unready, deadline).unwrap();
    assert_eq!(policy.incidents()[0].status, Status::Unrecovered);
}

/// Reconnection needs fresh evidence for unfinished work, while completed jobs need no new training activity.
#[test]
fn reconnect_evidence_requirements_distinguish_completed_and_unfinished_work() {
    for completed in [false, true] {
        let mut settings = config();
        settings.mode = Mode::ControllerAssisted;
        let (mut policy, worker) = detected_policy(settings);
        if completed {
            policy
                .event(&worker.agent_id, &event("completed", 0, 2, "one"), 4)
                .unwrap();
        }
        policy.transport_lost(5, "hub disconnected");
        assert!(policy.reserve_restart(&worker, 23).is_err());
        policy.transport_connected(24);
        let decision = policy.observe(&worker, Health::Unready, 24).unwrap();
        if completed {
            assert_eq!(decision, Decision::Restart);
            policy.reserve_restart(&worker, 24).unwrap();
            assert_eq!(policy.budget().restarts[&worker.deployment], 1);
        } else {
            assert_eq!(decision, Decision::Observe);
            assert_eq!(
                policy.report().assessments[&worker.deployment].reason,
                Reason::EvidenceRequired
            );
            assert!(policy.budget().restarts.is_empty());
        }
    }
}

/// Backoff changes later interventions, keeps the initial delay and never exceeds the configured cap.
#[test]
fn exponential_backoff_and_jitter_are_bounded_and_do_not_reset_budgets() {
    let mut settings = config();
    settings.mode = Mode::ControllerAssisted;
    settings.policy.max_restarts = 3;
    settings.resilience.backoff_multiplier = 2;
    settings.resilience.backoff_max_seconds = 60;
    settings.resilience.jitter_seconds = 3;
    for seed in 0..32 {
        assert_eq!(settings.resilience.cooldown(20, 0, "worker", seed), 20);
        assert!((20..=23).contains(&settings.resilience.cooldown(20, 1, "worker", seed)));
        assert!((40..=43).contains(&settings.resilience.cooldown(20, 2, "worker", seed)));
        assert_eq!(settings.resilience.cooldown(20, 20, "worker", seed), 60);
    }
    let (mut policy, worker) = detected_policy(settings);
    policy.reserve_restart(&worker, 23).unwrap();
    policy
        .action_result(
            &worker,
            Action::RestartFailed {
                error: "temporary API failure".into(),
            },
        )
        .unwrap();
    assert_eq!(policy.observe(&worker, Health::Unready, 42).unwrap(), Decision::Observe);
    policy.reserve_restart(&worker, 46).unwrap();
    policy.action_result(&worker, Action::RestartRequested).unwrap();
    assert_eq!(policy.observe(&worker, Health::Unready, 66).unwrap(), Decision::Observe);
    assert_eq!(policy.observe(&worker, Health::Unready, 89).unwrap(), Decision::Restart);
    assert_eq!(policy.budget().restarts[&worker.deployment], 2);
}

/// First useful recovery remains the MTTR boundary while stability requires subsequent progress or completion.
#[test]
fn stability_does_not_inflate_mttr_or_require_completed_work_to_run_again() {
    for completed in [false, true] {
        let mut settings = config();
        settings.resilience.stability_seconds = 5;
        let (mut policy, worker) = recovered_policy(settings);
        assert_eq!(policy.incidents()[0].recovered, ObservationTime::Recorded(5));
        let _quiet = policy.observe(&worker, Health::Ready, 10).unwrap();
        assert!(matches!(policy.incidents()[0].stability, Stability::Pending { .. }));
        let action = if completed { "completed" } else { "progress" };
        let progress = if completed { 1 } else { 2 };
        policy
            .event(&worker.agent_id, &event(action, progress, 3, "one"), 11)
            .unwrap();
        let _stable = policy.observe(&worker, Health::Ready, 12).unwrap();
        assert_eq!(policy.incidents()[0].stability, Stability::Stable { verified_at: 12 });
        assert_eq!(policy.report().recovery_seconds, vec![5]);
    }
}

/// Missing observations cannot certify stability; recurrent failure remains a separate incident.
#[test]
fn stability_records_unknown_observations_and_recurrent_failures() {
    for health in [Health::Unknown, Health::Unready] {
        let mut settings = config();
        settings.resilience.stability_seconds = 5;
        let (mut policy, worker) = recovered_policy(settings);
        let _next = policy.observe(&worker, health, 6).unwrap();
        let expected = if health == Health::Unknown {
            Stability::Incomplete
        } else {
            Stability::Unstable { observed_at: 6 }
        };
        assert_eq!(policy.incidents()[0].stability, expected);
        assert_eq!(policy.incidents()[0].recovered, ObservationTime::Recorded(5));
    }
}

/// Recover useful progress while retaining a five-second stability window for later observations.
fn pending_stability_policy() -> (Supervisor, Workload) {
    let mut settings = config();
    settings.resilience.stability_seconds = 5;
    recovered_policy(settings)
}

/// A ready deployment cannot certify stability after an application failure or an unexpected process replacement.
#[test]
fn stability_rejects_application_failure_and_replacement_even_when_the_deployment_is_ready() {
    for (kind, progress, sequence, instance) in [("failed", 1, 3, "one"), ("started", 1, 1, "replacement")] {
        let (mut policy, worker) = pending_stability_policy();
        policy
            .event(&worker.agent_id, &event(kind, progress, sequence, instance), 6)
            .unwrap();
        let _observed = policy.observe(&worker, Health::Ready, 6).unwrap();
        assert_eq!(policy.incidents()[0].stability, Stability::Unstable { observed_at: 6 });
        assert_eq!(policy.incidents()[0].status, Status::Recovered);
        assert_eq!(policy.incidents()[0].recovered, ObservationTime::Recorded(5));
        assert_eq!(policy.report().recovery_seconds, vec![5]);
    }
}

/// Fresh progress or completion cannot shorten the stability window; the exact boundary can certify recovery.
#[test]
fn early_progress_and_completion_wait_for_the_full_stability_window() {
    for (kind, progress) in [("progress", 2), ("completed", 1)] {
        let (mut policy, worker) = pending_stability_policy();
        policy
            .event(&worker.agent_id, &event(kind, progress, 3, "one"), 6)
            .unwrap();
        for now in [6, 9] {
            let _early = policy.observe(&worker, Health::Ready, now).unwrap();
            assert!(matches!(
                policy.incidents()[0].stability,
                Stability::Pending { since: 5, .. }
            ));
        }
        let _boundary = policy.observe(&worker, Health::Ready, 10).unwrap();
        assert_eq!(policy.incidents()[0].stability, Stability::Stable { verified_at: 10 });
        assert_eq!(policy.report().recovery_seconds, vec![5]);
    }
}

/// A hub disconnect or supervisor replacement cannot certify stability across a missing-observation interval.
#[test]
fn transport_loss_interrupts_stability_without_changing_the_recovery_time() {
    for restore in [false, true] {
        let mut settings = config();
        settings.resilience.stability_seconds = 5;
        let (mut policy, worker) = recovered_policy(settings.clone());
        assert!(matches!(policy.incidents()[0].stability, Stability::Pending { .. }));
        if restore {
            policy = Supervisor::restore(settings, report(&policy)).unwrap();
        } else {
            policy.transport_lost(6, "hub disconnected during stability verification");
        }
        assert_eq!(policy.incidents()[0].stability, Stability::Incomplete);
        assert_eq!(policy.incidents()[0].status, Status::Recovered);
        assert_eq!(policy.report().recovery_seconds, vec![5]);
        assert_eq!(policy.report().observation_gaps.len(), 1);
        let now = policy.elapsed(10);
        policy.transport_connected(now);
        policy
            .event(&worker.agent_id, &event("progress", 2, 3, "one"), now + 1)
            .unwrap();
        let _ready = policy.observe(&worker, Health::Ready, now + 2).unwrap();
        assert_eq!(policy.incidents()[0].stability, Stability::Incomplete);
        assert_eq!(policy.incidents().len(), 1);
        assert_eq!(policy.report().recovery_seconds, vec![5]);
    }
}

/// Transport loss fences interventions until renewed evidence, retaining sequence and instance protections.
#[test]
fn reconnect_requires_renewed_evidence_and_keeps_the_original_incident() {
    let mut settings = config();
    settings.mode = Mode::ControllerAssisted;
    let (mut policy, worker) = detected_policy(settings);
    policy.transport_lost(10, "hub disconnected");
    policy.transport_connected(30);
    assert_eq!(policy.observe(&worker, Health::Unready, 31).unwrap(), Decision::Observe);
    assert_eq!(
        policy.report().assessments[&worker.deployment].reason,
        Reason::EvidenceRequired
    );
    assert!(
        policy
            .event(&worker.agent_id, &event("started", 0, 1, "one"), 31)
            .is_err()
    );
    policy
        .event(&worker.agent_id, &event("progress", 0, 2, "one"), 32)
        .unwrap();
    assert_eq!(policy.observe(&worker, Health::Unready, 33).unwrap(), Decision::Restart);
    assert_eq!(policy.incidents().len(), 1);
    assert_eq!(policy.incidents()[0].onset, 0);
    assert_eq!(
        policy.report().observation_gaps[0].ended_at,
        ObservationTime::Recorded(30)
    );
    policy
        .event(&worker.agent_id, &event("progress", 1, 3, "one"), 34)
        .unwrap();
    policy.transport_lost(35, "another interruption");
    policy.transport_connected(36);
    let _stale = policy.observe(&worker, Health::Ready, 37).unwrap();
    assert_eq!(policy.incidents()[0].status, Status::Detected);
    policy
        .event(&worker.agent_id, &event("progress", 2, 4, "one"), 38)
        .unwrap();
    let _renewed = policy.observe(&worker, Health::Ready, 39).unwrap();
    assert_eq!(policy.incidents()[0].status, Status::Recovered);
}

/// Dependency permissions allow read-only observation but never authorize rollout mutation of supporting services.
#[tokio::test]
async fn supporting_dependency_is_not_an_intervention_target() {
    let mut settings = config();
    let _previous = settings
        .resilience
        .dependencies
        .insert("example-worker".into(), vec!["storage".into()]);
    let mut target = settings.workloads[0].clone();
    target.deployment = "storage".into();
    let backend = et_recovery_supervisor::kubernetes::Kubernetes::new(settings).unwrap();
    let error = backend.execute(&target, &["rollout", "restart"]).await.unwrap_err();
    assert!(error.to_string().contains("allowlisted"));
}

/// Additional read-only dependencies increase the allowed cycle duration without weakening ownership probes.
#[test]
fn health_budget_includes_dependency_observations() {
    let mut settings = config();
    let _previous = settings.resilience.dependencies.insert(
        "example-worker".into(),
        vec!["storage".into(), "example-worker-other".into()],
    );
    assert_eq!(et_recovery_supervisor::health::silence_limit(&settings).as_secs(), 52);
}
