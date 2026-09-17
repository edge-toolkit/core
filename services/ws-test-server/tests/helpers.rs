//! Exercises the `connect_agent` / `next_payload` client helpers' fail-fast and frame-skipping paths that the
//! happy-path hub tests never reach: the ack-wait timeout, protocol-ack skipping, and control-frame skipping. Each test
//! drives the helper against a tiny scripted ws server that emits an exact frame sequence, so the behaviour is
//! deterministic rather than dependent on real-hub timing.
//!
//! `wait_for_connected_agents` is covered here too, but against a real hub rather than a scripted server, because
//! what it reports is the registry's own view of who is connected -- the thing a caller gates on, and one no
//! scripted frame sequence would prove.
#![cfg(test)]

use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use edge_toolkit::ws::{ConnectStatus, ServerMessage};
use et_ws_test_server::{connect_agent, next_payload, start, wait_for_connected_agents};
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::{Bytes, Message};
use tokio_tungstenite::{accept_async, connect_async};

/// Budget for a wait that is expected to succeed; generous because only the failure paths are timing-sensitive.
const AMPLE: Duration = Duration::from_secs(10);

/// Register one agent against `ws_url` on a thread of its own and keep its socket open for `hold`.
///
/// The socket is polled throughout rather than merely held, so the hub's pings are answered and the agent stays
/// connected for as long as the test needs it. Joining the returned handle is what makes the agent go away.
fn peer_for(ws_url: &str, hold: Duration) -> JoinHandle<()> {
    let url = ws_url.to_owned();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let (mut socket, _agent_id) = connect_agent(&url).await;
            // Elapsed-versus-budget rather than a computed deadline: comparing two `Duration`s needs no
            // arithmetic on an `Instant`, which the workspace's restriction lints would otherwise object to.
            let started = tokio::time::Instant::now();
            while started.elapsed() < hold {
                let _frame = tokio::time::timeout(Duration::from_millis(100), socket.next()).await;
            }
        });
    })
}

#[test]
fn wait_for_connected_agents_sees_a_registered_peer() {
    let server = start();
    let peer = peer_for(&server.ws_url, Duration::from_secs(3));

    let seen = wait_for_connected_agents(&server.ws_url, 1, AMPLE);

    assert_eq!(seen.len(), 1, "expected the one connected peer, got {seen:?}");
    peer.join().unwrap();
}

#[test]
fn wait_for_connected_agents_gives_up_when_nobody_registers() {
    let server = start();

    // Nothing but the waiter itself ever connects, so this must run its budget out and report an empty roster
    // rather than counting the agent it had to register in order to ask.
    let seen = wait_for_connected_agents(&server.ws_url, 1, Duration::from_secs(2));

    assert!(seen.is_empty(), "the waiter must not count itself, got {seen:?}");
}

#[test]
fn wait_for_connected_agents_ignores_a_peer_that_has_gone() {
    let server = start();
    peer_for(&server.ws_url, Duration::from_millis(200)).join().unwrap();

    // The registry keeps listing a departed agent -- only its state changes -- and the hub needs a moment to
    // notice the closed socket, so poll until it has. Code that counted disconnected entries would never see an
    // empty roster here, because each round leaves its own waiter behind, and would fail on the deadline instead.
    let started = Instant::now();
    let mut seen = wait_for_connected_agents(&server.ws_url, 1, Duration::from_millis(500));
    while started.elapsed() < AMPLE && !seen.is_empty() {
        seen = wait_for_connected_agents(&server.ws_url, 1, Duration::from_millis(500));
    }

    assert!(
        seen.is_empty(),
        "a departed peer must not count as connected, got {seen:?}"
    );
}

/// Start a ws server on a free port that accepts one connection, sends `frames` in order, then holds the socket open.
///
/// With an empty `frames` it simply accepts and stays silent -- a server that never acks.
async fn scripted_server(frames: Vec<Message>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    // Detached scripted server; the handle is intentionally not joined (the task runs until the test ends).
    let _server = tokio::spawn(async move {
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
    });
    format!("ws://127.0.0.1:{port}")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[should_panic(expected = "et-connect-ack")]
async fn connect_agent_times_out_when_server_never_acks() {
    // The server accepts the socket but never sends `et-connect-ack`, so connect_agent must give up (panic) once its
    // bound elapses rather than hang the test indefinitely.
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
        Message::Ping(Bytes::new()),
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
