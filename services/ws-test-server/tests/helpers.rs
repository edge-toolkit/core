//! Exercises the `connect_agent` / `next_payload` client helpers' fail-fast and frame-skipping paths that the
//! happy-path hub tests never reach: the ack-wait timeout, protocol-ack skipping, and control-frame skipping.
//! Each test drives the helper against a tiny scripted ws server that emits an exact frame sequence, so the
//! behaviour is deterministic rather than dependent on real-hub timing.
#![cfg(test)]

use edge_toolkit::ws::{ConnectStatus, ServerMessage};
use et_ws_test_server::{connect_agent, next_payload};
use futures_util::SinkExt as _;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{accept_async, connect_async};

/// Start a ws server on a free port that accepts one connection, sends `frames` in order, then holds the
/// socket open. With an empty `frames` it simply accepts and stays silent -- a server that never acks.
async fn scripted_server(frames: Vec<Message>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let Ok(mut ws) = accept_async(stream).await else {
            return;
        };
        for frame in frames {
            if ws.send(frame).await.is_err() {
                return;
            }
        }
        // Keep the connection open so the client can finish reading rather than seeing an early close.
        std::future::pending::<()>().await;
    }));
    format!("ws://127.0.0.1:{port}")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[should_panic(expected = "et-connect-ack")]
async fn connect_agent_times_out_when_server_never_acks() {
    // The server accepts the socket but never sends `et-connect-ack`, so connect_agent must give up (panic)
    // once its bound elapses rather than hang the test indefinitely.
    let url = scripted_server(Vec::new()).await;
    let _connected = connect_agent(&url).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn next_payload_skips_control_frames_and_protocol_acks() {
    let ack = serde_json::to_string(&ServerMessage::ConnectAck {
        agent_id: "scripted".to_owned(),
        status: ConnectStatus::Assigned,
    })
    .unwrap();
    let frames = vec![
        // A control frame and a protocol ack both precede the real payload; next_payload must skip both.
        Message::Ping(Vec::new()),
        Message::text(ack),
        Message::text("actual-payload"),
    ];
    let url = scripted_server(frames).await;

    let (mut stream, _response) = connect_async(&url).await.unwrap();
    let payload = next_payload(&mut stream).await;
    let Message::Text(text) = payload else {
        panic!("expected the real text payload, got {payload:?}");
    };
    assert_eq!(
        text.as_str(),
        "actual-payload",
        "next_payload should return the first non-ack payload"
    );
}
