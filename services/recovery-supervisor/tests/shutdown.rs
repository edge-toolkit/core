//! Unix termination signals must save application evidence received since the last completed observation cycle.
#![cfg(test)]
#![cfg(unix)]

use std::time::Duration;

use edge_toolkit::ws::{ClientMessage, MessageDeliveryStatus, ServerMessage};
use et_recovery_supervisor::config::Config;
use et_test_recovery::{config, environment, event};
use fs_err as fs;
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::process::{Child, Command};
use tokio_tungstenite::tungstenite::Message;

/// Launch the production executable with isolated state and a kubeconfig that cannot contact a cluster.
#[expect(
    clippy::single_call_fn,
    reason = "keep subprocess setup separate from shutdown assertions"
)]
fn launch(config: &Config) -> Child {
    let fixture =
        edge_toolkit::config::get_project_root().join("services/recovery-supervisor/tests/fixtures/kubeconfig.yaml");
    Command::new(env!("CARGO_BIN_EXE_et-recovery-supervisor"))
        .envs(environment())
        .env("RECOVERY_WS_URL", &config.ws_url)
        .env("RECOVERY_STATE_FILE", &config.state_file)
        .env("RECOVERY_POLL_SECONDS", config.poll_seconds.to_string())
        .env("KUBECONFIG", fixture)
        .env("RUST_LOG", "warn")
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap()
}

/// Wait for the first completed poll so later evidence cannot be saved by a scheduled cycle.
#[expect(
    clippy::single_call_fn,
    reason = "isolate the bounded startup wait from the termination test"
)]
async fn wait_for_initial_poll(config: &Config, child: &mut Child) {
    tokio::time::timeout(Duration::from_secs(20), async {
        while et_recovery_supervisor::health::check(config).is_err() {
            assert!(
                child.try_wait().unwrap().is_none(),
                "supervisor exited before its initial poll"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

/// Require the child to process a replay after the valid checkpoint, rather than relying on queue admission alone.
#[expect(
    clippy::single_call_fn,
    reason = "log synchronization proves evidence was applied before signaling"
)]
async fn wait_for_replay_rejection(child: &mut Child) {
    let stdout = child.stdout.take().unwrap();
    let mut lines = BufReader::new(stdout).lines();
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(line) = lines.next_line().await.unwrap() {
            if line.contains("stale evidence") {
                return;
            }
        }
        panic!("supervisor exited before rejecting the replay");
    })
    .await
    .unwrap();
}

/// Deliver lifecycle evidence and require its own transport acknowledgement.
async fn send(socket: &mut et_ws_runner_common::RegisteredSocket, kind: &str, progress: u64, sequence: u64) {
    let message = ClientMessage::SendAgentMessage {
        to_agent_id: "recovery-supervisor".into(),
        message: serde_json::to_value(event(kind, progress, sequence, "one")).unwrap(),
    };
    socket
        .send(Message::Text(serde_json::to_string(&message).unwrap().into()))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(Ok(Message::Text(text))) = socket.next().await {
            if matches!(
                ServerMessage::from_text_frame(&text),
                Ok(ServerMessage::MessageStatus {
                    status: MessageDeliveryStatus::Acknowledged,
                    ..
                })
            ) {
                return;
            }
        }
        panic!("lifecycle delivery was not acknowledged");
    })
    .await
    .unwrap();
}

/// Save accepted but not yet polled checkpoint evidence and release the lease on the normal container-stop signal.
#[tokio::test]
async fn sigterm_saves_the_final_report_before_a_successful_exit() {
    let directory = tempfile::tempdir().unwrap();
    let server = et_ws_test_server::start();
    let mut config = config();
    config.ws_url.clone_from(&server.ws_url);
    config.state_file = directory.path().join("state.json");
    config.poll_seconds = 3600;
    let mut child = launch(&config);
    wait_for_initial_poll(&config, &mut child).await;
    let (mut producer, _) = et_ws_test_server::connect_agent_as(&server.ws_url, Some("example-worker")).await;
    send(&mut producer, "started", 0, 1).await;
    send(&mut producer, "checkpoint-restored", 3, 2).await;
    send(&mut producer, "checkpoint-restored", 3, 2).await;
    wait_for_replay_rejection(&mut child).await;
    let before: serde_json::Value = serde_json::from_slice(&fs::read(&config.state_file).unwrap()).unwrap();
    assert_eq!(before["checkpoint_measurements"], serde_json::json!([]));
    let signal = Command::new("coreutils")
        .args(["kill", "-TERM", &child.id().unwrap().to_string()])
        .output()
        .await
        .unwrap();
    assert!(signal.status.success(), "{signal:?}");
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(
        status.success(),
        "supervisor terminated without a graceful exit: {status}"
    );
    let after: serde_json::Value = serde_json::from_slice(&fs::read(&config.state_file).unwrap()).unwrap();
    assert_eq!(after["checkpoint_measurements"][0]["progress"], 3_u64);
    assert!(et_recovery_supervisor::health::check(&config).is_err());
}
