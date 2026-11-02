//! Probes require a live lease and fresh evidence from the current supervision session, without contacting Kubernetes.
#![cfg(test)]

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use edge_toolkit::ws::{ConnectStatus, MessageScope, ServerMessage};
use et_recovery_supervisor::RecoveryError;
use et_recovery_supervisor::config::Config;
use et_recovery_supervisor::health::{Session, check, silence_limit};
use et_recovery_supervisor::kubernetes::Backend;
use et_recovery_supervisor::policy::Health;
use et_test_recovery::{config, environment};
use fs_err as fs;
use futures_util::{SinkExt as _, StreamExt as _};
use tokio_tungstenite::tungstenite::Message;

/// Point a default configuration at an isolated temporary state file.
fn configured(directory: &Path) -> Config {
    let mut config = config();
    config.state_file = directory.join("state.json");
    config
}

/// Alter one heartbeat field to exercise health validation without changing the durable budget.
fn replace(config: &Config, field: &str, value: serde_json::Value) {
    let path = config.state_file.with_extension("health.json");
    let mut evidence: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    evidence[field] = value;
    fs::write(path, serde_json::to_vec(&evidence).unwrap()).unwrap();
}

/// Run the actual health-check executable with the fixture's state location.
fn probe(config: &Config) -> std::process::Output {
    // The unsuccessful exit is the assertion under test, not a command-launch failure.
    std::process::Command::new(env!("CARGO_BIN_EXE_et-recovery-healthcheck"))
        .envs(environment())
        .env("RECOVERY_STATE_FILE", &config.state_file)
        .output()
        .unwrap()
}

/// Allow an in-flight blocking save to release its lease, then require both health checks to reject the owner.
async fn assert_stopped(config: &Config) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !check(config).is_err_and(|error| error.to_string().contains("not running")) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(check(config).unwrap_err().to_string().contains("not running"));
    assert!(!probe(config).status.success());
}

/// Bound the wait for a supervisor to finish and publish a healthy observation cycle.
async fn wait_for_health(config: &Config) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while check(config).is_err() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

/// Keep startup unhealthy until a completed cycle and ensure probing cannot create or consume a restart budget.
#[tokio::test]
async fn startup_waits_for_a_completed_cycle_and_a_read_only_probe_cannot_spend_the_budget() {
    let directory = tempfile::tempdir().unwrap();
    let config = configured(directory.path());
    let _missing = check(&config).unwrap_err();
    assert!(!config.state_file.with_extension("lock").exists());
    let session = Session::acquire(&config).unwrap();
    session.maintain().await.unwrap();
    let _starting = check(&config).unwrap_err();
    assert!(!probe(&config).status.success());
    session.advance().await.unwrap();
    session.maintain().await.unwrap();
    check(&config).unwrap();
    assert!(probe(&config).status.success());
    assert!(!config.state_file.exists());
    drop(session);
    assert_stopped(&config).await;
}

/// Prevent a replacement supervisor from inheriting another process's healthy status.
#[tokio::test]
async fn a_new_owner_cannot_reuse_a_previous_sessions_fresh_heartbeat() {
    let directory = tempfile::tempdir().unwrap();
    let config = configured(directory.path());
    let first = Session::acquire(&config).unwrap();
    first.advance().await.unwrap();
    let identity = fs::read(config.state_file.with_extension("session")).unwrap();
    let _already_owned = Session::acquire(&config).unwrap_err();
    assert_eq!(fs::read(config.state_file.with_extension("session")).unwrap(), identity);
    check(&config).unwrap();
    drop(first);
    assert_stopped(&config).await;
    let second = Session::acquire(&config).unwrap();
    assert_ne!(fs::read(config.state_file.with_extension("session")).unwrap(), identity);
    assert!(
        check(&config)
            .unwrap_err()
            .to_string()
            .contains("current recovery session")
    );
    second.advance().await.unwrap();
    check(&config).unwrap();
}

/// Keep ownership exclusive while allowing a separate health-check process to read session metadata on every OS.
#[tokio::test]
async fn session_identity_remains_readable_without_reading_the_locked_lease() {
    let directory = tempfile::tempdir().unwrap();
    let config = configured(directory.path());
    let session = Session::acquire(&config).unwrap();
    let path = config.state_file.with_extension("session");
    let identity = fs::read_to_string(&path).unwrap();
    let _identity = uuid::Uuid::parse_str(&identity).unwrap();
    assert_eq!(fs::metadata(config.state_file.with_extension("lock")).unwrap().len(), 0);
    session.advance().await.unwrap();
    assert!(probe(&config).status.success());
    fs::remove_file(&path).unwrap();
    assert!(check(&config).is_err());
    assert!(!probe(&config).status.success());
    fs::write(path, identity).unwrap();
    check(&config).unwrap();
}

/// Release ownership if readable session metadata cannot be published, without advertising old evidence as healthy.
#[tokio::test]
async fn unavailable_session_storage_releases_the_budget_lease() {
    let directory = tempfile::tempdir().unwrap();
    let config = configured(directory.path());
    let path = config.state_file.with_extension("session");
    fs::create_dir(&path).unwrap();
    let _unavailable = Session::acquire(&config).unwrap_err();
    assert_stopped(&config).await;
    fs::remove_dir(path).unwrap();
    let _owner = Session::acquire(&config).unwrap();
}

/// Reject corrupted heartbeats and evidence belonging to another job, session or schema.
#[tokio::test]
async fn malformed_foreign_and_unknown_version_evidence_fails_closed() {
    let directory = tempfile::tempdir().unwrap();
    let config = configured(directory.path());
    let session = Session::acquire(&config).unwrap();
    for (field, value) in [
        ("schema_version", serde_json::json!(2_u32)),
        ("job_id", serde_json::json!("other-job")),
        ("session", serde_json::json!("other-session")),
    ] {
        session.advance().await.unwrap();
        replace(&config, field, value);
        assert!(
            check(&config)
                .unwrap_err()
                .to_string()
                .contains("current recovery session")
        );
    }
    fs::write(config.state_file.with_extension("health.json"), b"invalid json").unwrap();
    assert!(check(&config).unwrap_err().to_string().contains("JSON"));
    session.advance().await.unwrap();
    check(&config).unwrap();
}

/// Reject stale or future-dated evidence while allowing a newly completed cycle to restore health.
#[tokio::test]
async fn stale_cycles_and_future_timestamps_are_unhealthy_but_progress_restores_health() {
    let directory = tempfile::tempdir().unwrap();
    let config = configured(directory.path());
    let session = Session::acquire(&config).unwrap();
    for (timestamp, message) in [
        (SystemTime::UNIX_EPOCH, "stale"),
        (SystemTime::now() + Duration::from_secs(3600), "future"),
    ] {
        session.advance().await.unwrap();
        replace(&config, "completed_at", serde_json::to_value(timestamp).unwrap());
        assert!(check(&config).unwrap_err().to_string().contains(message));
        assert!(!probe(&config).status.success());
    }
    session.advance().await.unwrap();
    check(&config).unwrap();
}

/// Keep a failed heartbeat write from advertising a healthy supervision cycle.
#[tokio::test]
async fn unavailable_evidence_storage_does_not_publish_health() {
    let directory = tempfile::tempdir().unwrap();
    let config = configured(directory.path());
    let session = Session::acquire(&config).unwrap();
    fs::create_dir_all(config.state_file.with_extension("health.json")).unwrap();
    let _unwritable = session.advance().await.unwrap_err();
    let _missing = check(&config).unwrap_err();
}

/// Reject invalid settings and inaccessible lease files before supervision can claim ownership.
#[test]
fn invalid_configuration_and_unavailable_lock_storage_fail_before_startup() {
    let directory = tempfile::tempdir().unwrap();
    let mut config = configured(directory.path());
    config.poll_seconds = 0;
    let _invalid_owner = Session::acquire(&config).unwrap_err();
    let _invalid_probe = check(&config).unwrap_err();
    config.poll_seconds = 2;
    config.state_file = directory.path().join("absent/state.json");
    let _unavailable = Session::acquire(&config).unwrap_err();
}

/// Include every workload's bounded commands in the health allowance without integer overflow.
#[test]
fn silence_budget_covers_all_bounded_commands_and_polling_without_overflow() {
    let mut config = config();
    assert_eq!(silence_limit(&config), Duration::from_secs(32));
    config.workloads.push(config.workloads[0].clone());
    assert_eq!(silence_limit(&config), Duration::from_secs(52));
    config.command_timeout_seconds = u64::MAX;
    assert_eq!(silence_limit(&config), Duration::from_secs(u64::MAX));
}

#[derive(Default)]
struct PausedBackend {
    observations: u32,
    blocked: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

impl Backend for PausedBackend {
    /// Pause an observation on demand or report the configured API-failure condition.
    async fn health(&mut self, _workload: &et_recovery_supervisor::config::Workload) -> Result<Health, RecoveryError> {
        self.observations = self.observations.saturating_add(1);
        if self.observations == 2 {
            self.blocked.notify_one();
            self.release.notified().await;
        }
        Err(RecoveryError::Invalid("Kubernetes API unavailable".into()))
    }

    /// Reject unexpected recovery actions in health-probe tests.
    fn restart(
        &mut self,
        _workload: &et_recovery_supervisor::config::Workload,
    ) -> impl Future<Output = Result<(), RecoveryError>> + Send {
        std::future::ready(Err(RecoveryError::Invalid(
            "unknown observations must not trigger restarts".into(),
        )))
    }
}

/// Distinguish a stalled control loop from a completed cycle reporting an unavailable Kubernetes API.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_blocked_loop_cannot_refresh_health_and_api_failure_is_not_a_supervisor_stall() {
    let directory = tempfile::tempdir().unwrap();
    let server = et_ws_test_server::start();
    let mut config = configured(directory.path());
    config.ws_url.clone_from(&server.ws_url);
    config.poll_seconds = 1;
    let blocked = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let backend = PausedBackend {
        observations: 0,
        blocked: Arc::clone(&blocked),
        release: Arc::clone(&release),
    };
    let task = tokio::spawn(et_recovery_supervisor::runtime::run(config.clone(), backend));
    tokio::time::timeout(Duration::from_secs(20), blocked.notified())
        .await
        .unwrap();
    check(&config).unwrap();
    // Age the completed cycle deterministically while the actual observation loop is blocked.
    replace(
        &config,
        "completed_at",
        serde_json::to_value(SystemTime::UNIX_EPOCH).unwrap(),
    );
    assert!(check(&config).unwrap_err().to_string().contains("stale"));
    release.notify_one();
    wait_for_health(&config).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_stopped(&config).await;
}

/// Release health ownership on transport loss instead of trusting a recently written heartbeat.
#[tokio::test]
async fn graceful_and_abrupt_hub_loss_invalidate_health_even_with_a_fresh_heartbeat() {
    for graceful in [false, true] {
        assert_transport_failure(graceful).await;
    }
}

/// Verify owner shutdown and probe failure after one chosen hub-disconnection mode.
#[expect(
    clippy::single_call_fn,
    reason = "one transport-failure fixture exercises both connection shutdown modes"
)]
async fn assert_transport_failure(graceful: bool) {
    let directory = tempfile::tempdir().unwrap();
    let mut config = configured(directory.path());
    let (url, close, server) = fake_hub("recovery-supervisor", graceful).await;
    config.ws_url = url;
    config.resilience.reconnect = false;
    let backend = PausedBackend::default();
    let task = tokio::spawn(et_recovery_supervisor::runtime::run(config.clone(), backend));
    wait_for_health(&config).await;
    close.notify_one();
    let error = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("hub connection"));
    server.await.unwrap();
    assert_stopped(&config).await;
}

/// Serve a loopback protocol peer with controllable registration and shutdown behavior.
async fn fake_hub(identity: &str, graceful: bool) -> (String, Arc<tokio::sync::Notify>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    // Intentionally use plaintext on loopback only to test registration and shutdown, not TLS or production traffic.
    // nosemgrep: config.semgrep.no-semgrep-mentions -- external rule annotation
    // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket
    let url = format!("ws://{}/ws", listener.local_addr().unwrap());
    let identity = identity.to_owned();
    let close = Arc::new(tokio::sync::Notify::new());
    let shutdown = Arc::clone(&close);
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let _connect = socket.next().await.unwrap().unwrap();
        let acknowledgement = ServerMessage::ConnectAck {
            agent_id: identity.clone(),
            status: ConnectStatus::Assigned,
        };
        socket
            .send(Message::Text(serde_json::to_string(&acknowledgement).unwrap().into()))
            .await
            .unwrap();
        if identity == "recovery-supervisor" {
            let foreign = ServerMessage::AgentMessage {
                message_id: "foreign-evidence".into(),
                from_agent_id: "not-allowlisted".into(),
                scope: MessageScope::Direct,
                server_received_at: String::default(),
                message: serde_json::to_value(et_test_recovery::event("started", 0, 1, "foreign")).unwrap(),
            };
            socket
                .send(Message::Text(serde_json::to_string(&foreign).unwrap().into()))
                .await
                .unwrap();
        }
        shutdown.notified().await;
        if graceful {
            socket.close(None).await.unwrap();
        }
    });
    (url, close, server)
}

/// Release the lease after failed registration or heartbeat persistence.
#[tokio::test]
async fn registration_and_heartbeat_persistence_failures_never_leave_a_healthy_owner() {
    for bad_identity in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let mut config = configured(directory.path());
        let identity = if bad_identity {
            "wrong-supervisor"
        } else {
            "recovery-supervisor"
        };
        let (url, close, server) = fake_hub(identity, false).await;
        config.ws_url = url;
        config.resilience.reconnect = bad_identity;
        if !bad_identity {
            fs::create_dir_all(config.state_file.with_extension("health.json")).unwrap();
        }
        let backend = PausedBackend::default();
        let error = tokio::time::timeout(
            Duration::from_secs(5),
            et_recovery_supervisor::runtime::run(config.clone(), backend),
        )
        .await
        .unwrap()
        .unwrap_err();
        let expected = if bad_identity {
            "configured supervisor identity"
        } else {
            "state persistence failed"
        };
        assert!(error.to_string().contains(expected), "{error}");
        close.notify_one();
        server.await.unwrap();
        assert_stopped(&config).await;
    }
}

/// Retain queue-failure context when converting evidence-delivery errors.
#[test]
fn failed_evidence_delivery_preserves_the_transport_error() {
    for full in [false, true] {
        let evidence = (
            "example-worker".into(),
            et_test_recovery::event("started", 0, 1, "session"),
        );
        let delivery = if full {
            tokio::sync::mpsc::error::TrySendError::Full(evidence)
        } else {
            tokio::sync::mpsc::error::TrySendError::Closed(evidence)
        };
        let error = RecoveryError::from(delivery);
        assert!(error.to_string().contains("recovery evidence queue unavailable"));
    }
}
