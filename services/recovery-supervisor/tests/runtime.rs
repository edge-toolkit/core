//! Recovery actions are backend-neutral, persisted before execution, and carried over the real hub transport.
#![cfg(test)]

use std::time::Duration;

use edge_toolkit::ws::{ClientMessage, ServerMessage};
use et_recovery_supervisor::RecoveryError;
use et_recovery_supervisor::config::{Mode, Workload};
use et_recovery_supervisor::kubernetes::{Backend, Kubernetes};
use et_recovery_supervisor::persistence::StateStore;
use et_recovery_supervisor::policy::{Action, Health, Supervisor};
use et_recovery_supervisor::runtime::{cycle, hub, load_budget, load_supervisor, run};
use et_test_recovery::{config, environment, event, report, supervisor};
use fs_err as fs;
use futures_util::{SinkExt as _, StreamExt as _};
use tracing::field::Field;
use tracing::instrument::WithSubscriber as _;
use tracing::subscriber::set_default;
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::SubscriberExt as _;

/// Observe rejected evidence on the service task without installing a process-wide logging subscriber.
struct RejectedEvidence(tokio::sync::mpsc::UnboundedSender<String>);

impl<S: Subscriber> tracing_subscriber::Layer<S> for RejectedEvidence {
    /// Forward runtime error fields so the test can wait for policy rejection, not just transport acknowledgement.
    fn on_event(&self, event: &Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().target() != "et_recovery_supervisor::runtime" {
            return;
        }
        event.record(&mut |field: &Field, value: &dyn std::fmt::Debug| {
            if field.name() == "error" {
                self.0.send(format!("{value:?}")).unwrap();
            }
        });
    }
}

struct FakeBackend {
    config: et_recovery_supervisor::config::Config,
    restarts: u32,
    unknown: bool,
    fail_restart: bool,
}

impl Backend for FakeBackend {
    /// Return a controlled unhealthy or unavailable observation for policy tests.
    fn health(&mut self, _workload: &Workload) -> impl Future<Output = Result<Health, RecoveryError>> + Send {
        if self.unknown {
            return std::future::ready(Err(RecoveryError::Invalid("API unavailable".into())));
        }
        std::future::ready(Ok(Health::Unready))
    }

    /// Verify the durable reservation before recording a simulated restart attempt.
    fn restart(&mut self, workload: &Workload) -> impl Future<Output = Result<(), RecoveryError>> + Send {
        // The observable file must already contain the consumed budget before the backend can mutate anything.
        let restored = load_budget(&self.config).unwrap();
        assert_eq!(
            restored.restarts.get(&workload.deployment).copied(),
            Some(self.restarts.saturating_add(1))
        );
        self.restarts = self.restarts.saturating_add(1);
        if self.fail_restart {
            return std::future::ready(Err(RecoveryError::Invalid("restart denied".into())));
        }
        std::future::ready(Ok(()))
    }
}

/// Only an absent file starts a fresh supervisor; unreadable or invalid saved state fails closed.
#[tokio::test]
async fn supervisor_loading_preserves_committed_state_and_rejects_invalid_storage() {
    let directory = tempfile::tempdir().unwrap();
    let mut settings = config();
    settings.state_file = directory.path().join("state.json");
    let fresh = load_supervisor(&settings).unwrap();
    assert!(fresh.budget().restarts.is_empty());
    let mut saved = report(&fresh);
    saved["restart_budget"]["restarts"]["example-worker"] = serde_json::json!(1_u32);
    StateStore::new(settings.state_file.clone())
        .persist(saved)
        .await
        .unwrap();
    let restored = load_supervisor(&settings).unwrap();
    assert_eq!(restored.budget().restarts["example-worker"], 1);
    for bytes in [b"invalid json".as_slice(), b"{}".as_slice()] {
        fs::write(&settings.state_file, bytes).unwrap();
        let _invalid = load_supervisor(&settings).unwrap_err();
    }
    fs::remove_file(&settings.state_file).unwrap();
    fs::create_dir(&settings.state_file).unwrap();
    let _unreadable = load_supervisor(&settings).unwrap_err();
}

struct DependencyBackend {
    dependency_health: Health,
    observed: Vec<String>,
    restarted: Vec<String>,
}

impl Backend for DependencyBackend {
    /// Record every target so supporting services cannot be silently omitted or observed twice.
    fn health(&mut self, workload: &Workload) -> impl Future<Output = Result<Health, RecoveryError>> + Send {
        self.observed.push(workload.deployment.clone());
        std::future::ready(Ok(if workload.deployment == "storage" {
            self.dependency_health
        } else {
            Health::Unready
        }))
    }

    /// Record action targets to distinguish dependency observation from permission to restart it.
    fn restart(&mut self, workload: &Workload) -> impl Future<Output = Result<(), RecoveryError>> + Send {
        self.restarted.push(workload.deployment.clone());
        std::future::ready(Ok(()))
    }
}

/// Read dependencies before deciding on restarts, whether or not they are themselves supervised.
#[tokio::test]
async fn cycles_observe_dependencies_once_and_gate_actions_on_their_current_health() {
    for supervised in [false, true] {
        for health in [Health::Ready, Health::Unready, Health::Unknown] {
            let directory = tempfile::tempdir().unwrap();
            let mut settings = config();
            settings.mode = Mode::ControllerAssisted;
            settings.state_file = directory.path().join("state.json");
            let _previous = settings
                .resilience
                .dependencies
                .insert("example-worker".into(), vec!["storage".into()]);
            if supervised {
                let mut dependency = settings.workloads[0].clone();
                dependency.deployment = "storage".into();
                dependency.agent_id = "storage".into();
                settings.workloads.push(dependency);
            }
            let mut policy = Supervisor::new(settings.clone(), load_budget(&settings).unwrap()).unwrap();
            let store = StateStore::new(settings.state_file.clone());
            let mut backend = DependencyBackend {
                dependency_health: health,
                observed: Vec::new(),
                restarted: Vec::new(),
            };
            for now in [0, 3, 23] {
                cycle(&settings, &store, &mut policy, &mut backend, now).await.unwrap();
            }
            assert_eq!(backend.observed.iter().filter(|name| *name == "storage").count(), 3);
            assert_eq!(
                backend.restarted.contains(&"example-worker".into()),
                health == Health::Ready
            );
            if !supervised {
                assert!(!backend.restarted.contains(&"storage".into()));
                assert!(!policy.report().assessments.contains_key("storage"));
            }
        }
    }
}

/// Persist restart reservations before backend calls and retain attempts even when execution fails.
#[tokio::test]
async fn cycle_persists_budget_before_action_and_retains_failed_attempts() {
    let directory = tempfile::tempdir().unwrap();
    let (config, mut supervisor, mut backend) = assisted_fixture(directory.path());
    let store = StateStore::new(config.state_file.clone());
    for observed_at in [0, 3, 23] {
        cycle(&config, &store, &mut supervisor, &mut backend, observed_at)
            .await
            .unwrap();
    }
    backend.fail_restart = true;
    cycle(&config, &store, &mut supervisor, &mut backend, 43).await.unwrap();
    assert_eq!(backend.restarts, 2);
    let saved = load_budget(&config).unwrap();
    assert_eq!(saved.restarts["example-worker"], 2);
    assert_eq!(
        supervisor.incidents()[0].actions.last(),
        Some(&Action::RestartFailed {
            error: "restart denied".into()
        })
    );
    assert_eq!(
        report(&supervisor)["incidents"][0]["actions"][3]["kind"],
        "restart-failed"
    );
    backend.unknown = true;
    cycle(&config, &store, &mut supervisor, &mut backend, 63).await.unwrap();
    assert_eq!(supervisor.report().unknown_observations, 1_u64);
    assert_eq!(backend.restarts, 2);
}

/// Prevent external recovery actions when the durable budget cannot be committed.
#[tokio::test]
async fn persistence_failure_prevents_external_restart() {
    let directory = tempfile::tempdir().unwrap();
    let (config, mut supervisor, mut backend) = assisted_fixture(directory.path());
    let store = StateStore::new(config.state_file.clone());
    for observed_at in [0, 3] {
        cycle(&config, &store, &mut supervisor, &mut backend, observed_at)
            .await
            .unwrap();
    }
    fs::remove_file(&config.state_file).unwrap();
    fs::create_dir_all(&config.state_file).unwrap();
    let _error = cycle(&config, &store, &mut supervisor, &mut backend, 23)
        .await
        .unwrap_err();
    assert_eq!(backend.restarts, 0);
    let _error = load_budget(&config).unwrap_err();
}

/// Reject invalid restored budgets and verify that complete reports replace their predecessors.
#[tokio::test]
async fn malformed_state_fails_closed_and_atomic_updates_replace_previous_documents() {
    let directory = tempfile::tempdir().unwrap();
    let mut config = config();
    config.state_file = directory.path().join("state.json");
    let store = StateStore::new(config.state_file.clone());
    let mut supervisor = supervisor(Mode::ObserveOnly);
    supervisor
        .event("example-worker", &event("started", 0, 1, "one"), 0)
        .unwrap();
    for value in [serde_json::json!({}), serde_json::json!({"restart_budget":{}})] {
        store.persist(value).await.unwrap();
        let _error = load_budget(&config).unwrap_err();
    }
    fs::write(&config.state_file, b"invalid json").unwrap();
    let _error = load_budget(&config).unwrap_err();
    store.persist(supervisor.report()).await.unwrap();
    assert_eq!(load_budget(&config).unwrap().job_id, "example-job");
}

/// Preserve sender identity and acknowledge direct lifecycle messages over the real hub.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hub_delivers_sender_bound_cloudevents_and_acknowledges_receipt() {
    let _telemetry = et_otlp::init_guarded(None).unwrap();
    let server = et_ws_test_server::start();
    let (observer, _) = et_ws_test_server::connect_agent_as(&server.ws_url, Some("observer")).await;
    let (mut producer, producer_id) = et_ws_test_server::connect_agent_as(&server.ws_url, Some("example-worker")).await;
    let (sender, mut receiver) = tokio::sync::mpsc::channel(8);
    let task = tokio::spawn(hub(observer, sender));
    let message = ClientMessage::SendAgentMessage {
        to_agent_id: "observer".into(),
        message: serde_json::to_value(event("started", 0, 1, "one")).unwrap(),
    };
    send_message(&mut producer, &message).await;
    let (sender_id, evidence) = tokio::time::timeout(Duration::from_secs(3), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sender_id, producer_id);
    assert_eq!(evidence.event_type, "et.recovery.started");
    assert_acknowledged(&mut producer).await;
    task.abort();
}

/// Reject undeclared deployments before launching any Kubernetes command.
#[tokio::test]
async fn kubernetes_rejects_non_allowlisted_targets_before_running_a_command() {
    let backend = Kubernetes::new(config()).unwrap();
    let mut workload = config().workloads.remove(0);
    workload.deployment = "not-allowlisted".into();
    assert!(
        backend
            .execute(&workload, &["get", "-o", "json"])
            .await
            .unwrap_err()
            .to_string()
            .contains("allowlisted")
    );
}

struct ReadyBackend;

struct GatedBackend {
    observing: tokio::sync::mpsc::Sender<()>,
    release: tokio::sync::mpsc::Receiver<()>,
}

impl Backend for GatedBackend {
    /// Hold infrastructure observation until the test has queued acknowledged application evidence.
    async fn health(&mut self, _workload: &Workload) -> Result<Health, RecoveryError> {
        self.observing.send(()).await.unwrap();
        self.release.recv().await.unwrap();
        Ok(Health::Ready)
    }

    /// Preserve the healthy fixture's rejection of unexpected recovery actions.
    async fn restart(&mut self, workload: &Workload) -> Result<(), RecoveryError> {
        ReadyBackend.restart(workload).await
    }
}

/// Retain evidence until capacity returns without requiring the producer to resend it, and fail on consumer loss.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_evidence_queue_waits_for_capacity_without_requiring_a_resend() {
    let server = et_ws_test_server::start();
    let (observer, _) = et_ws_test_server::connect_agent_as(&server.ws_url, Some("observer")).await;
    let (mut producer, _) = et_ws_test_server::connect_agent_as(&server.ws_url, Some("example-worker")).await;
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    sender
        .try_send(("occupied".into(), event("started", 0, 1, "one")))
        .unwrap();
    let task = tokio::spawn(hub(observer, sender));
    let message = ClientMessage::SendAgentMessage {
        to_agent_id: "observer".into(),
        message: serde_json::to_value(event("progress", 1, 2, "one")).unwrap(),
    };
    send_message(&mut producer, &message).await;
    assert!(!task.is_finished());
    assert_eq!(receiver.try_recv().unwrap().0, "occupied");
    assert_acknowledged(&mut producer).await;
    let (identity, evidence) = receiver.try_recv().unwrap();
    assert_eq!(identity, "example-worker");
    assert_eq!(evidence.event_type, "et.recovery.progress");
    drop(receiver);
    send_message(&mut producer, &message).await;
    let error = tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("recovery evidence queue unavailable"));
}

/// Exercise real kubectl context errors using an isolated fixture without contacting a cluster.
#[tokio::test]
async fn kubernetes_reports_command_errors_without_contacting_a_cluster() {
    let _logging = set_default(tracing_subscriber::fmt().with_test_writer().finish());
    let mut config = config();
    config.command_timeout_seconds = 60;
    let workload = config.workloads[0].clone();
    let backend = Kubernetes::new(config).unwrap();
    let path =
        edge_toolkit::config::get_project_root().join("services/recovery-supervisor/tests/fixtures/kubeconfig.yaml");
    let fixture = path.to_str().unwrap();
    let error = backend
        .execute(&workload, &["--kubeconfig", fixture, "get", "-o", "json"])
        .await
        .unwrap_err();
    assert!(
        matches!(error, RecoveryError::Invalid(_)),
        "expected a kubectl context error, not a process startup or timeout failure: {error}"
    );
    assert!(error.to_string().contains("kubectl"));
    assert!(error.to_string().contains("context"));
}

/// Reject an unusable budget directory before creating state or supervising workloads.
#[test]
fn binary_fails_closed_on_an_unavailable_state_directory() {
    let directory = tempfile::tempdir().unwrap();
    let mut config = config();
    config.state_file = directory.path().join("absent").join("state.json");
    // Raw output is intentional: an unsuccessful exit is the outcome under test.
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_et-recovery-supervisor"))
        .envs(environment())
        .env("RECOVERY_STATE_FILE", &config.state_file)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr).unwrap().contains("Io("));
    assert!(!config.state_file.exists());
}

impl Backend for ReadyBackend {
    /// Report a ready deployment while testing lifecycle transport.
    fn health(&mut self, _workload: &Workload) -> impl Future<Output = Result<Health, RecoveryError>> + Send {
        std::future::ready(Ok(Health::Ready))
    }

    /// Reject any unexpected restart of a healthy fixture.
    fn restart(&mut self, _workload: &Workload) -> impl Future<Output = Result<(), RecoveryError>> + Send {
        std::future::ready(Err(RecoveryError::Invalid("unexpected restart of healthy work".into())))
    }
}

/// Reject foreign and replayed evidence while accepting valid work between polls and persisting it on disconnect.
#[tokio::test]
async fn idle_control_rejects_invalid_evidence_without_losing_subsequent_valid_work() {
    let directory = tempfile::tempdir().unwrap();
    let server = et_ws_test_server::start();
    let mut config = connected_config(&server, directory.path(), 3600);
    config.resilience.reconnect = false;
    let (sender, mut rejected) = tokio::sync::mpsc::unbounded_channel();
    let subscriber = tracing_subscriber::registry().with(RejectedEvidence(sender));
    let task = tokio::spawn(run(config.clone(), ReadyBackend).with_subscriber(subscriber));
    let (mut producer, _) = et_ws_test_server::connect_agent_as(&server.ws_url, Some("example-worker")).await;
    wait_for_agent(&mut producer, &config.agent_id).await;
    wait_for_completed_cycle(&config).await;
    let mut foreign = event("checkpoint-restored", 999, 1, "foreign");
    foreign.data["job_id"] = "another-job".into();
    send_evidence(&mut producer, &config.agent_id, foreign).await;
    assert_rejection(&mut rejected, "invalid lifecycle identity or job").await;
    let started = event("started", 0, 1, "one");
    let checkpoint = event("checkpoint-restored", 3, 2, "one");
    send_evidence(&mut producer, &config.agent_id, started).await;
    send_evidence(&mut producer, &config.agent_id, checkpoint.clone()).await;
    send_evidence(&mut producer, &config.agent_id, checkpoint).await;
    assert_rejection(&mut rejected, "stale evidence").await;
    assert!(!task.is_finished());
    server.stop().await;
    let stopped = tokio::time::timeout(Duration::from_secs(5), task).await.unwrap();
    let error = stopped.unwrap().unwrap_err();
    assert!(matches!(error, RecoveryError::Socket(_) | RecoveryError::Invalid(_)));
    let saved: serde_json::Value = serde_json::from_slice(&fs::read(&config.state_file).unwrap()).unwrap();
    let checkpoints = saved["checkpoint_measurements"].as_array().unwrap();
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(checkpoints[0]["progress"], 3_u64);
    assert_eq!(checkpoints[0]["instance"], "one");
    assert_eq!(saved["incidents"], serde_json::json!([]));
}

/// A replacement hub at the same endpoint restores supervision without replacing its owner or resetting state.
#[tokio::test]
async fn supervisor_reconnects_to_a_replacement_hub_and_retains_lifecycle_context() {
    let directory = tempfile::tempdir().unwrap();
    let server = et_ws_test_server::start();
    let port = url::Url::parse(&server.ws_url).unwrap().port().unwrap();
    let mut config = connected_config(&server, directory.path(), 1);
    config.resilience.connect_timeout_seconds = 1;
    let task = tokio::spawn(run(config.clone(), ReadyBackend));
    let (mut producer, _) = et_ws_test_server::connect_agent_as(&server.ws_url, Some("example-worker")).await;
    wait_for_agent(&mut producer, &config.agent_id).await;
    send_evidence(&mut producer, &config.agent_id, event("started", 0, 1, "one")).await;
    wait_for_sequence(&config, 1).await;
    server.stop().await;
    wait_for_registration_retries(&config).await;
    assert!(!task.is_finished());
    let replacement = et_ws_test_server::start_on(port);
    let (mut producer, _) = et_ws_test_server::connect_agent_as(&replacement.ws_url, Some("example-worker")).await;
    wait_for_agent(&mut producer, &config.agent_id).await;
    send_evidence(&mut producer, &config.agent_id, event("progress", 1, 2, "one")).await;
    wait_for_sequence(&config, 2).await;
    assert!(!task.is_finished());
    let saved: serde_json::Value = serde_json::from_slice(&fs::read(&config.state_file).unwrap()).unwrap();
    assert!(saved["observation_gaps"].as_array().unwrap().last().unwrap()["ended_at"].is_number());
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
}

/// Leave the hub unavailable across bounded registration attempts and observe their persisted gap.
#[expect(
    clippy::single_call_fn,
    reason = "registration retry waiting is distinct from sequence advancement"
)]
async fn wait_for_registration_retries(config: &et_recovery_supervisor::config::Config) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let saved: serde_json::Value = serde_json::from_slice(&fs::read(&config.state_file).unwrap()).unwrap();
            if saved["context"]["elapsed_seconds"].as_u64().unwrap() >= 3
                && saved["observation_gaps"]
                    .as_array()
                    .unwrap()
                    .last()
                    .is_some_and(|gap| gap["ended_at"].is_null())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

/// Observe persisted sequence advancement rather than assume that a transport acknowledgement committed policy state.
async fn wait_for_sequence(config: &et_recovery_supervisor::config::Config, expected: u64) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let saved: serde_json::Value = serde_json::from_slice(&fs::read(&config.state_file).unwrap()).unwrap();
            if saved["context"]["states"]["example-worker"]["sequence"] == expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

/// Complete registration and the initial poll before sending events that must be handled by the idle control loop.
#[expect(
    clippy::single_call_fn,
    reason = "completed-cycle waiting separates poll and idle-receive coverage"
)]
async fn wait_for_completed_cycle(config: &et_recovery_supervisor::config::Config) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while et_recovery_supervisor::health::check(config).is_err() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

/// Require a fresh rejection for each invalid event rather than reusing an earlier warning.
async fn assert_rejection(receiver: &mut tokio::sync::mpsc::UnboundedReceiver<String>, expected: &str) {
    let error = tokio::time::timeout(Duration::from_secs(5), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(error.contains(expected), "expected {expected}: {error}");
}

/// Deliver one lifecycle event and wait for its transport acknowledgement independently of policy acceptance.
async fn send_evidence(
    producer: &mut et_ws_runner_common::RegisteredSocket,
    identity: &str,
    evidence: edge_toolkit::ws::CloudEvent,
) {
    let message = ClientMessage::SendAgentMessage {
        to_agent_id: identity.into(),
        message: serde_json::to_value(evidence).unwrap(),
    };
    send_message(producer, &message).await;
    assert_acknowledged(producer).await;
}

/// Give each real-hub test an isolated budget path and an explicit observation interval.
fn connected_config(
    server: &et_ws_test_server::TestServer,
    directory: &std::path::Path,
    poll_seconds: u64,
) -> et_recovery_supervisor::config::Config {
    let mut config = config();
    config.ws_url.clone_from(&server.ws_url);
    config.poll_seconds = poll_seconds;
    config.state_file = directory.join("state.json");
    config
}

/// Consume real hub evidence while preventing concurrent ownership of the same durable budget.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn service_consumes_lifecycle_evidence_and_excludes_a_second_budget_owner() {
    let directory = tempfile::tempdir().unwrap();
    let server = et_ws_test_server::start();
    let config = connected_config(&server, directory.path(), 1);
    let (observing, mut observed) = tokio::sync::mpsc::channel(1);
    let (release, gate) = tokio::sync::mpsc::channel(1);
    let task = tokio::spawn(run(
        config.clone(),
        GatedBackend {
            observing,
            release: gate,
        },
    ));
    let (mut producer, _) = et_ws_test_server::connect_agent_as(&server.ws_url, Some("example-worker")).await;
    wait_for_agent(&mut producer, &config.agent_id).await;
    tokio::time::timeout(Duration::from_secs(5), observed.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        run(config.clone(), ReadyBackend)
            .await
            .unwrap_err()
            .to_string()
            .contains("budget lock")
    );
    for (kind, sequence) in [("started", 1), ("checkpoint-restored", 2), ("progress", 3)] {
        let message = ClientMessage::SendAgentMessage {
            to_agent_id: config.agent_id.clone(),
            message: serde_json::to_value(event(kind, 1, sequence, "one")).unwrap(),
        };
        send_message(&mut producer, &message).await;
        assert_acknowledged(&mut producer).await;
    }
    release.send(()).await.unwrap();
    wait_for_checkpoint(&config).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
}

/// Require evidence queued during the blocked observation to appear in the first persisted cycle.
#[expect(
    clippy::single_call_fn,
    reason = "bounded report waiting is separate from transport and ownership assertions"
)]
async fn wait_for_checkpoint(config: &et_recovery_supervisor::config::Config) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let document: serde_json::Value = serde_json::from_slice(&fs::read(&config.state_file).unwrap()).unwrap();
            if document["checkpoint_measurements"]
                .as_array()
                .is_some_and(|values| values.len() == 1)
            {
                assert_eq!(document["checkpoint_measurements"][0]["progress"], 1_u64);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

/// Build a controller-assisted policy and backend sharing one isolated durable budget.
fn assisted_fixture(directory: &std::path::Path) -> (et_recovery_supervisor::config::Config, Supervisor, FakeBackend) {
    let mut config = config();
    config.mode = Mode::ControllerAssisted;
    config.state_file = directory.join("state.json");
    let supervisor = Supervisor::new(config.clone(), load_budget(&config).unwrap()).unwrap();
    let backend = FakeBackend {
        config: config.clone(),
        restarts: 0,
        unknown: false,
        fail_restart: false,
    };
    (config, supervisor, backend)
}

/// Serialize and send one typed client message through an established hub connection.
async fn send_message(socket: &mut et_ws_runner_common::RegisteredSocket, message: &ClientMessage) {
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(message).unwrap().into(),
        ))
        .await
        .unwrap();
}

/// Require an acknowledged delivery status within a bounded transport wait.
async fn assert_acknowledged(producer: &mut et_ws_runner_common::RegisteredSocket) {
    let acknowledged = tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(Ok(frame)) = producer.next().await {
            if let tokio_tungstenite::tungstenite::Message::Text(text) = frame
                && matches!(
                    ServerMessage::from_text_frame(&text),
                    Ok(ServerMessage::MessageStatus {
                        status: edge_toolkit::ws::MessageDeliveryStatus::Acknowledged,
                        ..
                    })
                )
            {
                return true;
            }
        }
        false
    })
    .await
    .unwrap();
    assert!(acknowledged);
}

/// Wait for the supervisor's registered identity before sending workload evidence.
async fn wait_for_agent(producer: &mut et_ws_runner_common::RegisteredSocket, identity: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            producer
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    serde_json::to_string(&ClientMessage::ListAgents).unwrap().into(),
                ))
                .await
                .unwrap();
            if let tokio_tungstenite::tungstenite::Message::Text(text) = et_ws_test_server::next_payload(producer).await
                && let Ok(ServerMessage::ListAgentsResponse { agents }) = ServerMessage::from_text_frame(&text)
                && agents.iter().any(|agent| agent.agent_id == identity)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
