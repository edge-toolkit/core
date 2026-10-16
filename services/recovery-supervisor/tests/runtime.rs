//! Recovery actions are backend-neutral, persisted before execution, and carried over the real hub transport.
#![cfg(test)]

use std::time::Duration;

use edge_toolkit::ws::{ClientMessage, ServerMessage};
use et_recovery_supervisor::RecoveryError;
use et_recovery_supervisor::config::{Mode, Workload};
use et_recovery_supervisor::kubernetes::{Backend, Kubernetes};
use et_recovery_supervisor::persistence::StateStore;
use et_recovery_supervisor::policy::{Action, Health, Supervisor};
use et_recovery_supervisor::runtime::{cycle, hub, load_budget, run};
use et_test_recovery::{config, event, report, supervisor};
use futures_util::{SinkExt as _, StreamExt as _};

struct FakeBackend {
    config: et_recovery_supervisor::config::Config,
    restarts: u32,
    unknown: bool,
    fail_restart: bool,
}

impl Backend for FakeBackend {
    fn health(&mut self, _workload: &Workload) -> impl Future<Output = Result<Health, RecoveryError>> + Send {
        if self.unknown {
            return std::future::ready(Err(RecoveryError::Invalid("API unavailable".into())));
        }
        std::future::ready(Ok(Health::Unready))
    }

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
    std::fs::remove_file(&config.state_file).unwrap();
    std::fs::create_dir_all(&config.state_file).unwrap();
    let _error = cycle(&config, &store, &mut supervisor, &mut backend, 23)
        .await
        .unwrap_err();
    assert_eq!(backend.restarts, 0);
    let _error = load_budget(&config).unwrap_err();
}

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
    std::fs::write(&config.state_file, b"invalid json").unwrap();
    let _error = load_budget(&config).unwrap_err();
    store.persist(supervisor.report()).await.unwrap();
    assert_eq!(load_budget(&config).unwrap().job_id, "example-job");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hub_delivers_sender_bound_cloudevents_and_acknowledges_receipt() {
    let _telemetry = et_otlp::init_guarded(None).unwrap();
    let server = et_ws_test_server::start();
    let (observer, _, _) = et_ws_runner_common::connect_and_register(
        &server.ws_url,
        Some("observer".into()),
        Some(Duration::from_secs(30)),
    )
    .await
    .unwrap();
    let (mut producer, producer_id, _) = et_ws_runner_common::connect_and_register(
        &server.ws_url,
        Some("example-worker".into()),
        Some(Duration::from_secs(3)),
    )
    .await
    .unwrap();
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
    let acknowledged = tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(Ok(frame)) = producer.next().await {
            if let tokio_tungstenite::tungstenite::Message::Text(text) = frame
                && let Ok(ServerMessage::MessageStatus {
                    status: edge_toolkit::ws::MessageDeliveryStatus::Acknowledged,
                    ..
                }) = ServerMessage::from_text_frame(&text)
            {
                return true;
            }
        }
        false
    })
    .await
    .unwrap();
    assert!(acknowledged);
    task.abort();
}

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

#[tokio::test]
async fn kubernetes_reports_command_errors_without_contacting_a_cluster() {
    let backend = Kubernetes::new(config()).unwrap();
    let workload = config().workloads.remove(0);
    let path =
        edge_toolkit::config::get_project_root().join("services/recovery-supervisor/tests/fixtures/kubeconfig.yaml");
    let fixture = path.to_str().unwrap();
    let error = backend
        .execute(&workload, &["--kubeconfig", fixture, "get", "-o", "json"])
        .await
        .unwrap_err();
    assert!(error.to_string().contains("kubectl"));
    assert!(error.to_string().contains("context"));
}

#[test]
fn binary_fails_closed_on_an_unavailable_state_directory() {
    let directory = tempfile::tempdir().unwrap();
    let mut config = config();
    config.state_file = directory.path().join("absent").join("state.json");
    let input = directory.path().join("recovery.toml");
    std::fs::write(&input, toml::to_string(&config).unwrap()).unwrap();
    // Raw output is intentional: an unsuccessful exit is the outcome under test.
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_et-recovery-supervisor"))
        .arg(&input)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_ne!(output.stderr, Vec::<u8>::new());
    assert!(!config.state_file.exists());
}

impl Backend for ReadyBackend {
    fn health(&mut self, _workload: &Workload) -> impl Future<Output = Result<Health, RecoveryError>> + Send {
        std::future::ready(Ok(Health::Ready))
    }

    fn restart(&mut self, _workload: &Workload) -> impl Future<Output = Result<(), RecoveryError>> + Send {
        std::future::ready(Err(RecoveryError::Invalid("unexpected restart of healthy work".into())))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn service_consumes_lifecycle_evidence_and_excludes_a_second_budget_owner() {
    let directory = tempfile::tempdir().unwrap();
    let server = et_ws_test_server::start();
    let mut config = config();
    config.ws_url.clone_from(&server.ws_url);
    config.poll_seconds = 1;
    config.state_file = directory.path().join("state.json");
    let task = tokio::spawn(run(config.clone(), ReadyBackend));
    let (mut producer, _) = et_ws_test_server::connect_agent_as(&server.ws_url, Some("example-worker")).await;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            producer
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    serde_json::to_string(&ClientMessage::ListAgents).unwrap().into(),
                ))
                .await
                .unwrap();
            if let tokio_tungstenite::tungstenite::Message::Text(text) =
                et_ws_test_server::next_payload(&mut producer).await
                && let Ok(ServerMessage::ListAgentsResponse { agents }) = ServerMessage::from_text_frame(&text)
                && agents.iter().any(|agent| agent.agent_id == config.agent_id)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
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
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let document: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&config.state_file).unwrap()).unwrap();
            if document["checkpoint_measurements"]
                .as_array()
                .is_some_and(|values| values.len() == 1)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
    let _cancelled = task.await;
}

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

async fn send_message(socket: &mut et_ws_runner_common::RegisteredSocket, message: &ClientMessage) {
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(message).unwrap().into(),
        ))
        .await
        .unwrap();
}
