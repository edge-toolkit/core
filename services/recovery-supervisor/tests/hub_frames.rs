//! Exercise control frames and malformed application payloads against the supervisor's real WebSocket loop.
#![cfg(test)]

use std::time::Duration;

use edge_toolkit::ws::{ClientMessage, MessageScope, ServerMessage};
use et_recovery_supervisor::RecoveryError;
use et_recovery_supervisor::config::Mode;
use et_recovery_supervisor::policy::{Health, Status};
use et_recovery_supervisor::runtime::hub;
use et_test_recovery::{config, event, supervisor};
use et_ws_runner_common::RegisteredSocket;
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::io::AsyncWriteExt as _;
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

/// Establish a real loopback transport without requiring a hub registry or Kubernetes.
async fn peer() -> (RegisteredSocket, WebSocketStream<TcpStream>) {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    // Intentionally use plaintext on loopback only to exercise WebSocket frames without unrelated TLS
    // negotiation.
    // nosemgrep: config.semgrep.no-semgrep-mentions -- external rule annotation
    // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket
    let endpoint = format!("ws://{}/ws", listener.local_addr().unwrap());
    let client = tokio::spawn(async move { tokio_tungstenite::connect_async(endpoint).await.unwrap().0 });
    let (stream, _) = listener.accept().await.unwrap();
    let server = tokio_tungstenite::accept_async(stream).await.unwrap();
    (client.await.unwrap(), server)
}

/// Read one protocol response with a bound so missing acknowledgements cannot hang a test.
async fn receive(server: &mut WebSocketStream<TcpStream>) -> Message {
    tokio::time::timeout(Duration::from_secs(3), server.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}

/// Require the transport task to finish with a service error rather than hang or panic.
async fn stopped(task: tokio::task::JoinHandle<Result<(), RecoveryError>>) -> RecoveryError {
    let outcome = tokio::time::timeout(Duration::from_secs(3), task).await.unwrap();
    outcome.unwrap().unwrap_err()
}

/// Send a direct message without assuming the bounded evidence queue can accept it immediately.
async fn send_direct(server: &mut WebSocketStream<TcpStream>, identity: &str, payload: serde_json::Value) {
    let message = ServerMessage::AgentMessage {
        from_agent_id: "example-worker".into(),
        message_id: identity.into(),
        scope: MessageScope::Direct,
        message: payload,
        server_received_at: String::default(),
    };
    server
        .send(Message::Text(serde_json::to_string(&message).unwrap().into()))
        .await
        .unwrap();
}

/// Require the acknowledgement for this message rather than an earlier successful delivery.
async fn acknowledged(server: &mut WebSocketStream<TcpStream>, identity: &str) {
    let Message::Text(response) = receive(server).await else {
        panic!("expected a direct-message acknowledgement");
    };
    assert!(matches!(
        serde_json::from_str::<ClientMessage>(&response).unwrap(),
        ClientMessage::MessageAck { message_id } if message_id == identity
    ));
}

/// Deliver a direct message and require its own acknowledgement, even if its event payload is invalid.
async fn direct(server: &mut WebSocketStream<TcpStream>, identity: &str, payload: serde_json::Value) {
    send_direct(server, identity, payload).await;
    acknowledged(server, identity).await;
}

/// Preserve replacement startup and later progress under overload while keeping outbound heartbeats active.
#[tokio::test]
async fn queue_backpressure_preserves_replacement_identity_and_recovery_progress() {
    let (client, mut server) = peer().await;
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    sender
        .try_send(("occupied".into(), event("started", 0, 1, "old")))
        .unwrap();
    let task = tokio::spawn(hub(client, sender));
    let mut policy = supervisor(Mode::ObserveOnly);
    policy
        .event("example-worker", &event("started", 0, 1, "old"), 0)
        .unwrap();
    let worker = config().workloads.remove(0);
    let _suspected = policy.observe(&worker, Health::Unready, 10).unwrap();
    let _detected = policy.observe(&worker, Health::Unready, 13).unwrap();
    for (identity, kind, progress, sequence) in [
        ("replacement-started", "started", 0, 1),
        ("replacement-progress", "progress", 1, 2),
    ] {
        let payload = serde_json::to_value(event(kind, progress, sequence, "new")).unwrap();
        send_direct(&mut server, identity, payload).await;
    }
    let heartbeat = tokio::time::timeout(Duration::from_secs(15), server.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(&heartbeat, Message::Ping(_)), "{heartbeat:?}");
    assert!(!task.is_finished());
    assert_eq!(receiver.try_recv().unwrap().0, "occupied");
    for (identity, now) in [("replacement-started", 14), ("replacement-progress", 15)] {
        let (sender, evidence) = tokio::time::timeout(Duration::from_secs(3), receiver.recv())
            .await
            .unwrap()
            .unwrap();
        policy.event(&sender, &evidence, now).unwrap();
        acknowledged(&mut server, identity).await;
    }
    let _recovered = policy.observe(&worker, Health::Ready, 16).unwrap();
    assert_eq!(policy.incidents()[0].status, Status::Recovered);
    server.close(None).await.unwrap();
    assert!(stopped(task).await.to_string().contains("hub connection closed"));
}

/// Acknowledge malformed direct events, answer pings and ignore unrelated frames without losing later valid events.
#[tokio::test]
async fn malformed_events_and_non_event_frames_do_not_stop_delivery() {
    let (client, mut server) = peer().await;
    let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
    let task = tokio::spawn(hub(client, sender));
    server.send(Message::Text("not-json".into())).await.unwrap();
    server.send(Message::Pong(vec![1_u8].into())).await.unwrap();
    server.send(Message::Binary(vec![2_u8].into())).await.unwrap();
    direct(&mut server, "invalid", serde_json::json!({"not":"a lifecycle event"})).await;
    assert_eq!(
        receiver.try_recv().unwrap_err(),
        tokio::sync::mpsc::error::TryRecvError::Empty
    );
    server.send(Message::Ping(vec![3_u8].into())).await.unwrap();
    assert_eq!(receive(&mut server).await, Message::Pong(vec![3_u8].into()));
    direct(
        &mut server,
        "valid",
        serde_json::to_value(et_test_recovery::event("progress", 1, 2, "worker")).unwrap(),
    )
    .await;
    let (identity, event) = receiver.try_recv().unwrap();
    assert_eq!(identity, "example-worker");
    assert_eq!(event.event_type, "et.recovery.progress");
    server.close(None).await.unwrap();
    let error = stopped(task).await;
    assert!(error.to_string().contains("hub connection closed"));
}

/// Preserve protocol errors instead of presenting a malformed WebSocket frame as a clean shutdown.
#[tokio::test]
async fn invalid_websocket_frames_preserve_the_transport_error() {
    let (client, mut server) = peer().await;
    let (sender, _receiver) = tokio::sync::mpsc::channel(4);
    let task = tokio::spawn(hub(client, sender));
    // Opcode 3 is reserved; sending raw bytes deliberately bypasses tungstenite's valid-frame encoder.
    server.get_mut().write_all(&[0x83_u8, 0_u8]).await.unwrap();
    let error = stopped(task).await;
    assert!(matches!(error, RecoveryError::Socket(_)), "{error}");
}
