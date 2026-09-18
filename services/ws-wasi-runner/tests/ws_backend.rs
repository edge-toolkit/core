//! Host websocket backend: the binary relay in both directions, and the heartbeat.
//!
//! `modules.rs` drives whole guests end to end, but no bundled guest sends or receives a binary relay frame,
//! and none sits still long enough for the 5s heartbeat to tick -- so all three paths were uncovered. The
//! inbound and heartbeat ones live in tasks `WsBackend::connect` spawns and are unreachable through the guest
//! API, hence driving the backend directly here against an in-process ws-server.
#![cfg(test)]
#![expect(
    clippy::arithmetic_side_effects,
    reason = "integration test: the deadline arithmetic cannot overflow within a test's lifetime"
)]

use std::time::Duration;

use edge_toolkit::ws::{ClientMessage, ServerMessage};
use et_ws_wasi_runner::HostState;
use et_ws_wasi_runner::bindings::et::ws_messages::messages::{ClientMessage as WitClientMessage, RelayBinaryPayload};
use et_ws_wasi_runner::bindings::et::ws_wasi::ws::{Host as _, State};
use et_ws_wasi_runner::host::ws::WsBackend;
use futures_util::{SinkExt as _, StreamExt as _};
use tokio_tungstenite::{connect_async, tungstenite};

/// Handshake budget for both the backend and the raw peer.
const ACK_TIMEOUT: Duration = Duration::from_secs(10);
/// How long to wait for a relayed frame to come back round through the hub.
const RELAY_TIMEOUT: Duration = Duration::from_secs(10);

/// Connect a plain websocket client and complete et-connect, returning the registered socket.
///
/// Deliberately not `WsBackend`: the point is to have a second agent the hub will relay *to*, driven with raw
/// frames, so the backend under test is the only thing being measured.
async fn connect_peer(
    ws_url: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let (mut socket, _response) = connect_async(ws_url).await.unwrap();
    let connect = serde_json::to_string(&ClientMessage::Connect { agent_id: None }).unwrap();
    socket.send(tungstenite::Message::text(connect)).await.unwrap();

    let deadline = tokio::time::Instant::now() + ACK_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline - tokio::time::Instant::now();
        let Ok(Some(Ok(frame))) = tokio::time::timeout(remaining, socket.next()).await else {
            break;
        };
        let tungstenite::Message::Text(text) = frame else {
            continue;
        };
        if matches!(
            serde_json::from_str::<ServerMessage>(&text),
            Ok(ServerMessage::ConnectAck { .. })
        ) {
            return socket;
        }
    }
    panic!("peer never received its et-connect-ack");
}

/// A binary frame from another agent reaches the guest inbox as `RelayBinary`.
///
/// The hub forwards any frame it cannot parse as a known `ClientMessage` verbatim, so a raw binary payload
/// lands on the reader pump's `Message::Binary` arm -- the one that calls `from_binary_frame`.
#[tokio::test(flavor = "current_thread")]
async fn binary_frame_arrives_as_relay_binary() {
    let server = et_ws_test_server::start();
    let backend = WsBackend::connect(&server.ws_url, Some(ACK_TIMEOUT)).await.unwrap();
    let mut peer = connect_peer(&server.ws_url).await;

    let payload = b"\x00\x01\x02 binary relay \xff".to_vec();
    peer.send(tungstenite::Message::binary(payload.clone())).await.unwrap();

    // The ack the handshake already consumed is re-surfaced as the first inbox message, so skip past it.
    let deadline = tokio::time::Instant::now() + RELAY_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline - tokio::time::Instant::now();
        let Some(message) = backend.next_message(remaining).await else {
            break;
        };
        if let ServerMessage::RelayBinary { content } = message {
            assert_eq!(
                content, payload,
                "the relayed bytes must survive the round trip unchanged"
            );
            return;
        }
    }
    panic!("no RelayBinary reached the guest inbox");
}

/// The heartbeat keeps the connection open past the server's idle-close window.
///
/// The server only bumps `last_activity` on inbound frames and closes an idle connection at 15s, so a backend
/// whose pinger never ticked is shut by the time this assertion runs. Staying `Connected` is therefore direct
/// evidence the pinger loop ran and its `Ping` was accepted as activity.
#[tokio::test(flavor = "current_thread")]
async fn heartbeat_keeps_the_connection_past_the_idle_close() {
    let server = et_ws_test_server::start();
    let backend = WsBackend::connect(&server.ws_url, Some(ACK_TIMEOUT)).await.unwrap();

    // Comfortably past the 15s idle close, with nothing else sent on the socket in the meantime.
    tokio::time::sleep(Duration::from_secs(18)).await;

    assert_eq!(
        backend.current_state().await,
        State::Connected,
        "an idle backend must be held open by its own heartbeat"
    );
}

/// A guest-sent `relay-binary` leaves as a raw binary frame another agent receives verbatim.
///
/// The outbound counterpart to the test above, and it goes through `HostState` rather than `WsBackend` because
/// the WIT-to-Rust conversion and the relay-vs-typed-JSON choice both live in `<HostState as Host>::send`.
/// Constructing the state needs no wasmtime store -- it is a plain struct of the URLs and the REST client.
#[tokio::test(flavor = "current_thread")]
async fn relay_binary_leaves_as_a_raw_binary_frame() {
    let server = et_ws_test_server::start();
    let mut peer = connect_peer(&server.ws_url).await;
    let mut state = HostState::new(&server.base_url, server.ws_url.clone(), Some(ACK_TIMEOUT), false);
    state.connect().await.unwrap();

    let payload = b"\x10\x20 outbound relay \xfe".to_vec();
    state
        .send(WitClientMessage::RelayBinary(RelayBinaryPayload {
            content: payload.clone(),
        }))
        .await
        .unwrap();

    let deadline = tokio::time::Instant::now() + RELAY_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline - tokio::time::Instant::now();
        let Ok(Some(Ok(frame))) = tokio::time::timeout(remaining, peer.next()).await else {
            break;
        };
        if let tungstenite::Message::Binary(bytes) = frame {
            assert_eq!(
                bytes.to_vec(),
                payload,
                "the relay must not wrap or re-encode the bytes"
            );
            return;
        }
    }
    panic!("the peer never received the relayed binary frame");
}
