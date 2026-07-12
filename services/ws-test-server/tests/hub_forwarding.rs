//! Verifies the ws service's hub-style fallback: any frame the server
//! can't parse as a known `ClientMessage` is forwarded verbatim to every
//! other connected agent. Covers both text and binary payloads.

#![cfg(test)]
#![expect(
    clippy::arithmetic_side_effects,
    clippy::needless_continue,
    clippy::similar_names,
    clippy::wildcard_enum_match_arm,
    reason = "integration tests: idiomatic test-time control flow"
)]

use std::time::Duration;

use edge_toolkit::ws::{ClientMessage, ServerMessage};
use futures_util::{SinkExt as _, StreamExt as _};
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// Open a ws connection, send `et-connect`, and return `(stream, agent_id)`
/// once `et-connect-ack` has been observed.
async fn connect_agent(
    ws_url: &str,
) -> (
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    String,
) {
    let (mut stream, _) = connect_async(ws_url).await.unwrap();
    let connect_msg = serde_json::to_string(&ClientMessage::Connect { agent_id: None }).unwrap();
    stream.send(Message::text(connect_msg)).await.unwrap();

    while let Some(msg) = stream.next().await {
        let msg = msg.unwrap();
        let Message::Text(text) = msg else {
            continue;
        };
        if let Ok(ServerMessage::ConnectAck { agent_id, .. }) = serde_json::from_str::<ServerMessage>(&text) {
            return (stream, agent_id);
        }
    }
    panic!("never received et-connect-ack");
}

/// Pull the next frame from `stream`, ignoring known protocol acks
/// (`et-message-status`, `et-connect-ack`, etc.) so callers see the
/// next "real" payload.
async fn next_payload(
    stream: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
) -> Message {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let next = tokio::time::timeout(remaining, stream.next()).await.unwrap();
        let msg = next.unwrap().unwrap();
        match &msg {
            Message::Text(text) => {
                if let Ok(parsed) = serde_json::from_str::<ServerMessage>(text)
                    && matches!(
                        parsed,
                        ServerMessage::ConnectAck { .. }
                            | ServerMessage::MessageStatus { .. }
                            | ServerMessage::Response { .. }
                    )
                {
                    continue;
                }
                return msg;
            }
            Message::Binary(_) => return msg,
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("unexpected control frame: {other:?}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unrecognised_text_is_broadcast_verbatim() {
    let server = et_ws_test_server::start();

    let (mut sender, _sender_id) = connect_agent(&server.ws_url).await;
    let (mut receiver, _receiver_id) = connect_agent(&server.ws_url).await;

    // A frame the server can't parse as ClientMessage -- no `type` field, no
    // recognisable shape. The hub fallback should forward it verbatim.
    let raw = r#"{"hello":"world","nested":{"n":42}}"#;
    sender.send(Message::text(raw)).await.unwrap();

    let received = next_payload(&mut receiver).await;
    let Message::Text(received_text) = received else {
        panic!("expected text frame, got {received:?}");
    };
    assert_eq!(
        received_text.as_str(),
        raw,
        "text payload must be forwarded byte-for-byte"
    );

    // Sender should not echo back to itself.
    let echoed = tokio::time::timeout(Duration::from_millis(300), sender.next()).await;
    assert!(
        echoed.is_err() || matches!(&echoed, Ok(Some(Ok(Message::Ping(_) | Message::Pong(_))))),
        "sender should not receive its own broadcast, got {echoed:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unrecognised_binary_is_broadcast_verbatim() {
    let server = et_ws_test_server::start();

    let (mut sender, _sender_id) = connect_agent(&server.ws_url).await;
    let (mut receiver, _receiver_id) = connect_agent(&server.ws_url).await;

    // Arbitrary opaque bytes -- the server has no way to interpret these,
    // so the hub fallback must forward them as-is.
    let payload: Vec<u8> = vec![0x00, 0x01, 0x02, 0xff, 0xfe, 0xfd, b'a', b'b', b'c'];
    sender.send(Message::binary(payload.clone())).await.unwrap();

    let received = next_payload(&mut receiver).await;
    let Message::Binary(received_bytes) = received else {
        panic!("expected binary frame, got {received:?}");
    };
    assert_eq!(
        &*received_bytes,
        payload.as_slice(),
        "binary payload must be forwarded byte-for-byte"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unconnected_client_is_auto_registered_and_relays_both_ways() {
    let server = et_ws_test_server::start();

    // A peer that speaks the et-protocol, standing in for the agent runner.
    let (mut peer, _peer_id) = connect_agent(&server.ws_url).await;
    // A "dumb" client that never sends et-connect -- e.g. a demo frontend on
    // a raw `new WebSocket(url)`.
    let (mut dumb, _) = connect_async(&server.ws_url).await.unwrap();

    // The dumb client's first binary frame must be broadcast to the peer:
    // sending it auto-registers the dumb client as an agent.
    let activations: Vec<u8> = vec![0x10, 0x20, 0x30, 0x40];
    dumb.send(Message::binary(activations.clone())).await.unwrap();

    let received = next_payload(&mut peer).await;
    let Message::Binary(received_bytes) = received else {
        panic!("expected binary frame at peer, got {received:?}");
    };
    assert_eq!(
        &*received_bytes,
        activations.as_slice(),
        "frame from unconnected client must be broadcast to peers"
    );

    // Reverse direction: the peer's reply must reach the now auto-registered
    // dumb client -- it became a broadcast recipient on its first frame.
    let grads: Vec<u8> = vec![0xaa, 0xbb, 0xcc];
    peer.send(Message::binary(grads.clone())).await.unwrap();

    let reply = next_payload(&mut dumb).await;
    let Message::Binary(reply_bytes) = reply else {
        panic!("expected binary reply at dumb client, got {reply:?}");
    };
    assert_eq!(
        &*reply_bytes,
        grads.as_slice(),
        "auto-registered client must receive peer broadcasts"
    );
}
