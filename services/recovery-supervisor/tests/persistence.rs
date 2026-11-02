//! Durable recovery context survives restoration, malformed input and cancelled asynchronous writes.
#![cfg(test)]

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use et_recovery_supervisor::config::Mode;
use et_recovery_supervisor::persistence::StateStore;
use et_recovery_supervisor::policy::{Action, Budget, Decision, Health, Status, Supervisor};
use et_test_recovery::{config, event, report, supervisor};
use fs_err as fs;
use serde::{Serialize, Serializer};
use tokio::sync::Notify;

/// Future saved timestamps reject startup with actionable context and leave the durable state unchanged.
#[test]
fn future_saved_at_names_the_timestamp_and_preserves_the_state_file() {
    let directory = tempfile::tempdir().unwrap();
    let mut settings = config();
    settings.state_file = directory.path().join("recovery.json");
    let policy = Supervisor::new(settings.clone(), Budget::new(settings.job_id.clone())).unwrap();
    let future = SystemTime::now().checked_add(Duration::from_secs(3600)).unwrap();
    let mut saved = report(&policy);
    saved["context"]["saved_at"] = serde_json::to_value(future).unwrap();
    let bytes = serde_json::to_vec(&saved).unwrap();
    fs::write(&settings.state_file, &bytes).unwrap();
    for _ in 0..2_u8 {
        let error = et_recovery_supervisor::runtime::load_supervisor(&settings).unwrap_err();
        assert!(error.to_string().contains(&format!("{future:?}")));
        assert!(error.to_string().contains("saved_at"));
        assert!(error.to_string().contains("correct the system clock"));
        assert_eq!(fs::read(&settings.state_file).unwrap(), bytes);
    }
}

/// Time spent stopped counts toward the original incident deadline and cannot grant another restart attempt.
#[test]
fn restored_downtime_expires_the_original_recovery_deadline() {
    let mut settings = config();
    settings.mode = Mode::ControllerAssisted;
    let worker = settings.workloads[0].clone();
    let mut policy = Supervisor::new(settings.clone(), Budget::new(settings.job_id.clone())).unwrap();
    for now in [0, 3] {
        let _observed = policy.observe(&worker, Health::Unready, now).unwrap();
    }
    policy.reserve_restart(&worker, 23).unwrap();
    let mut saved = report(&policy);
    saved["context"]["saved_at"] = serde_json::to_value(SystemTime::now() - Duration::from_secs(300)).unwrap();
    let mut restored = Supervisor::restore(settings, saved).unwrap();
    let now = restored.elapsed(0);
    assert!(now >= 323);
    assert_eq!(
        restored.observe(&worker, Health::Unready, now).unwrap(),
        Decision::Observe
    );
    assert_eq!(restored.incidents()[0].status, Status::Unrecovered);
    assert!(restored.reserve_restart(&worker, now).is_err());
    assert_eq!(restored.budget().restarts[&worker.deployment], 1);
}

struct PausedDocument {
    started: Arc<Notify>,
    release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
}

impl Serialize for PausedDocument {
    /// Pause serialization until the test releases the blocking writer.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.started.notify_one();
        let release = match self.release.lock() {
            Ok(release) => release,
            Err(error) => return Err(serde::ser::Error::custom(error)),
        };
        match release.recv_timeout(Duration::from_secs(5)) {
            Ok(()) => serializer.serialize_u64(1),
            Err(error) => Err(serde::ser::Error::custom(error)),
        }
    }
}

/// Keep cancelled callers from allowing older blocking writes to overtake later state commits.
#[tokio::test]
async fn cancellation_cannot_let_an_older_save_overwrite_the_final_report() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.json");
    let store = StateStore::new(path.clone());
    let release = cancelled_write(store.clone()).await;
    let mut final_save = tokio::spawn(async move { store.persist(2_u64).await });
    let _blocked = tokio::time::timeout(Duration::from_millis(20), &mut final_save)
        .await
        .unwrap_err();
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), final_save)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"2");
}

/// Cancel the async waiter only after serialization has entered the owned blocking write.
async fn cancelled_write(writer: StateStore) -> std::sync::mpsc::Sender<()> {
    let started = Arc::new(Notify::new());
    let (release, receiver) = std::sync::mpsc::channel();
    let paused = PausedDocument {
        started: Arc::clone(&started),
        release: std::sync::Mutex::new(receiver),
    };
    let first = tokio::spawn(async move { writer.persist(paused).await });
    // This single-thread runtime can still process notifications while serialization is deliberately blocked.
    tokio::time::timeout(Duration::from_secs(3), started.notified())
        .await
        .unwrap();
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    release
}

struct InvalidDocument;

/// An unfinished cancelled save retains ownership, preventing an old report from overwriting a replacement's context.
#[tokio::test]
async fn cancelled_persistence_keeps_the_owner_until_the_blocking_write_finishes() {
    let directory = tempfile::tempdir().unwrap();
    let mut config = et_test_recovery::config();
    config.state_file = directory.path().join("state.json");
    let session = et_recovery_supervisor::health::Session::acquire(&config).unwrap();
    let store = session.state_store(config.state_file.clone());
    let release = cancelled_write(store).await;
    drop(session);
    let _owned = et_recovery_supervisor::health::Session::acquire(&config).unwrap_err();
    release.send(()).unwrap();
    let owner = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match et_recovery_supervisor::health::Session::acquire(&config) {
                Ok(owner) => break owner,
                Err(et_recovery_supervisor::RecoveryError::Lease(_)) => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(error) => panic!("replacement could not acquire ownership: {error}"),
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(fs::read(&config.state_file).unwrap(), b"1");
    drop(owner);
}

impl Serialize for InvalidDocument {
    /// Inject a serialization failure before a replacement document can be committed.
    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Err(serde::ser::Error::custom("intentional serialization failure"))
    }
}

/// Preserve committed state after serialization failure and permit a subsequent successful save.
#[tokio::test]
async fn failed_serialization_preserves_the_last_document_and_releases_the_writer() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.json");
    let store = StateStore::new(path.clone());
    store.persist(1_u64).await.unwrap();
    let error = store.persist(InvalidDocument).await.unwrap_err();
    assert!(error.to_string().contains("intentional serialization failure"));
    assert_eq!(fs::read(&path).unwrap(), b"1");
    store.persist(2_u64).await.unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"2");
}

/// Durable context retains incidents, consumed attempts, identities and cooldowns across a process replacement.
#[test]
fn restored_context_keeps_policy_history_and_rejects_replayed_or_mismatched_evidence() {
    let mut settings = config();
    settings.mode = Mode::ControllerAssisted;
    let worker = settings.workloads[0].clone();
    let mut policy = supervisor(settings.mode);
    policy
        .event(&worker.agent_id, &event("started", 0, 1, "one"), 0)
        .unwrap();
    let _suspected = policy.observe(&worker, Health::Unready, 0).unwrap();
    let _detected = policy.observe(&worker, Health::Unready, 3).unwrap();
    policy.reserve_restart(&worker, 23).unwrap();
    policy.action_result(&worker, Action::RestartRequested).unwrap();
    let value = report(&policy);
    let mut restored = Supervisor::restore(settings.clone(), value.clone()).unwrap();
    assert_eq!(restored.incidents()[0].actions, policy.incidents()[0].actions);
    assert_eq!(restored.budget().restarts[&worker.deployment], 1);
    assert!(
        restored
            .event(&worker.agent_id, &event("started", 0, 1, "one"), restored.elapsed(1))
            .is_err()
    );
    assert_eq!(
        restored.observe(&worker, Health::Unready, restored.elapsed(1)).unwrap(),
        Decision::Observe
    );
    assert_eq!(restored.report().observation_gaps.len(), 1);
    settings.policy.max_restarts = 3;
    let _mismatched = Supervisor::restore(settings, value).unwrap_err();
}

/// Legacy budgets survive migration; malformed contexts and impossible observer clocks fail closed.
#[test]
fn restoration_archives_legacy_reports_and_rejects_invalid_contexts() {
    let settings = config();
    let policy = supervisor(Mode::ObserveOnly);
    let mut legacy = report(&policy);
    let _context = legacy.as_object_mut().unwrap().remove("context");
    let restored = Supervisor::restore(settings.clone(), legacy).unwrap();
    assert_eq!(
        report(&restored)["context"]["archived_reports"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    for (field, value) in [
        ("schema_version", serde_json::json!(2_u32)),
        ("states", serde_json::json!({})),
        (
            "saved_at",
            serde_json::json!({"secs_since_epoch": u64::MAX, "nanos_since_epoch": 0_u32}),
        ),
    ] {
        let mut invalid = report(&policy);
        invalid["context"][field] = value;
        let _invalid = Supervisor::restore(settings.clone(), invalid).unwrap_err();
    }
    let _missing = Supervisor::restore(settings, serde_json::json!({})).unwrap_err();
}

/// Corrupt saved budgets fail closed instead of creating a fresh supervisor with a reset restart allowance.
#[test]
fn restoration_rejects_malformed_restart_budgets() {
    for budget in [
        serde_json::Value::Null,
        serde_json::json!({"restarts": {"example-worker": 1_u32}}),
    ] {
        let error = Supervisor::restore(config(), serde_json::json!({"restart_budget": budget})).unwrap_err();
        assert!(matches!(error, et_recovery_supervisor::RecoveryError::Json(_)));
    }
}

/// A valid budget cannot authorise restoration of an unsupported report version or a different policy.
#[test]
fn restoration_checks_report_schema_and_configuration_even_with_matching_identity() {
    let policy = supervisor(Mode::ObserveOnly);
    let mut invalid_schema = report(&policy);
    invalid_schema["schema_version"] = serde_json::json!(2_u32);
    let mut changed_configuration = report(&policy);
    changed_configuration["context"]["configuration"]["policy"]["max_restarts"] = serde_json::json!(3_u32);
    for invalid in [invalid_schema, changed_configuration] {
        let error = Supervisor::restore(config(), invalid).unwrap_err();
        assert!(error.to_string().contains("does not match configuration"));
    }
}

/// Contradictory report identity cannot be restored even when its embedded configuration is valid.
#[test]
fn restoration_rejects_a_report_from_another_job_or_recovery_mode() {
    let policy = supervisor(Mode::ObserveOnly);
    for (field, value) in [("job_id", "foreign"), ("mode", "controller-assisted")] {
        let mut invalid = report(&policy);
        invalid[field] = serde_json::json!(value);
        let _foreign = Supervisor::restore(config(), invalid).unwrap_err();
    }
}

/// A persisted workload phase cannot refer outside the incident list or to another workload's incident.
#[test]
fn restoration_rejects_invalid_incident_references() {
    let mut policy = supervisor(Mode::ObserveOnly);
    let worker = config().workloads.remove(0);
    let _suspected = policy.observe(&worker, Health::Unready, 0).unwrap();
    let _detected = policy.observe(&worker, Health::Unready, 3).unwrap();
    let valid = report(&policy);
    let mut invalid_index = valid.clone();
    invalid_index["context"]["states"][&worker.deployment]["phase"]["Incident"] = serde_json::json!(999_u64);
    let error = Supervisor::restore(config(), invalid_index).unwrap_err();
    assert!(error.to_string().contains("saved incident index is invalid"));
    let mut invalid_workload = valid;
    invalid_workload["incidents"][0]["workload"] = serde_json::json!("foreign-worker");
    let error = Supervisor::restore(config(), invalid_workload).unwrap_err();
    assert!(error.to_string().contains("saved incident workload is inconsistent"));
}
