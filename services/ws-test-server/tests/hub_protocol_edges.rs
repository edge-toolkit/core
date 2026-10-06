//! The hub's answers to protocol edges: requests before `et-connect`, frames it cannot decode, acks for unknown ids,
//! direct messages to an agent that has gone away, and a binary relay that arrives as a text frame.
#![cfg(test)]

use std::time::Duration;

use edge_toolkit::ws::{AgentConnectionState, MessageDeliveryStatus, ServerMessage};
use et_ws_test_server::{connect_agent, connect_agent_as, next_payload, start};
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::{Bytes, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

/// A client connection to the hub.
type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Decode a text frame the hub sent, panicking on anything else.
fn server_message(frame: &Message) -> ServerMessage {
    let Message::Text(text) = frame else {
        panic!("expected a text frame, got {frame:?}");
    };
    serde_json::from_str(text).unwrap()
}

/// Send `body` as one text frame.
async fn send_json(socket: &mut Socket, body: &serde_json::Value) {
    socket.send(Message::text(body.to_string())).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requests_before_connect_are_answered_invalid() {
    let server = start();
    let (mut socket, _response) = connect_async(&server.ws_url).await.unwrap();

    let requests = [
        serde_json::json!({"type": "et-send-agent-message", "to_agent_id": "someone", "message": {}}),
        serde_json::json!({"type": "et-broadcast-message", "message": {}}),
        serde_json::json!({"type": "et-message-ack", "message_id": "anything"}),
    ];
    for request in &requests {
        send_json(&mut socket, request).await;
        let reply = server_message(&next_payload(&mut socket).await);
        let ServerMessage::Invalid { message_id, detail } = reply else {
            panic!("expected an et-invalid reply, got {reply:?}");
        };
        assert_eq!(message_id, None);
        assert!(detail.starts_with("agent must connect before "));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_undecodable_frame_and_a_pong_leave_the_connection_working() {
    let server = start();
    let (mut agent, _agent_id) = connect_agent(&server.ws_url).await;

    send_json(&mut agent, &serde_json::json!({"type": "et-alive"})).await;
    agent.send(Message::Pong(Bytes::new())).await.unwrap();
    assert_still_serving(&mut agent).await;
}

/// An `et-client-event` frame carrying a `CloudEvent` of `event_type` at `specversion`.
fn client_event(specversion: &str, event_type: &str, data: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "type": "et-client-event",
        "event": {
            "data": data,
            "id": "event-1",
            "source": "/modules/test",
            "specversion": specversion,
            "time": "2026-10-06T00:00:00Z",
            "type": event_type,
        },
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_events_are_logged_without_a_reply() {
    let server = start();
    let (mut agent, _agent_id) = connect_agent(&server.ws_url).await;

    let details = serde_json::json!({"detected_class": "person", "confidence": 0.9_f64, "processed_at": "now"});
    send_json(&mut agent, &client_event("1.0", "et.video_cv.inference", &details)).await;
    let loaded = serde_json::json!({"build": "test"});
    send_json(&mut agent, &client_event("1.0", "et.app.loaded", &loaded)).await;
    assert_still_serving(&mut agent).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_event_at_an_unsupported_specversion_is_answered_invalid() {
    let server = start();
    let (mut agent, _agent_id) = connect_agent(&server.ws_url).await;

    send_json(
        &mut agent,
        &client_event("0.3", "et.app.loaded", &serde_json::json!({})),
    )
    .await;
    let reply = server_message(&next_payload(&mut agent).await);
    let ServerMessage::Invalid { message_id, detail } = reply else {
        panic!("expected an et-invalid reply, got {reply:?}");
    };
    assert_eq!(message_id.as_deref(), Some("event-1"));
    assert_eq!(detail, r#"unsupported CloudEvents specversion "0.3"; expected "1.0""#);
}

/// Ask for the roster and require the hub to answer it, proving the connection survived what was sent before.
async fn assert_still_serving(agent: &mut Socket) {
    send_json(agent, &serde_json::json!({"type": "et-list-agents"})).await;
    let reply = server_message(&next_payload(agent).await);
    assert!(matches!(reply, ServerMessage::ListAgentsResponse { .. }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ack_for_an_unknown_message_is_answered_invalid() {
    let server = start();
    let (mut agent, _agent_id) = connect_agent(&server.ws_url).await;

    send_json(
        &mut agent,
        &serde_json::json!({"type": "et-message-ack", "message_id": "no-such-message"}),
    )
    .await;

    let reply = server_message(&next_payload(&mut agent).await);
    let ServerMessage::Invalid { message_id, .. } = reply else {
        panic!("expected an et-invalid reply, got {reply:?}");
    };
    assert_eq!(message_id.as_deref(), Some("no-such-message"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_message_for_a_disconnected_agent_is_queued_then_delivered_on_reconnect() {
    let server = start();
    let (mut sender, sender_id) = connect_agent(&server.ws_url).await;
    let (mut recipient, recipient_id) = connect_agent(&server.ws_url).await;
    recipient.close(None).await.unwrap();
    drop(recipient);
    wait_until_disconnected(&mut sender, &recipient_id).await;

    let send =
        serde_json::json!({"type": "et-send-agent-message", "to_agent_id": recipient_id, "message": {"n": 1_u32}});
    send_json(&mut sender, &send).await;
    assert_eq!(next_status(&mut sender).await, MessageDeliveryStatus::Queued);

    let (mut returned, _same_id) = connect_agent_as(&server.ws_url, Some(&recipient_id)).await;
    let delivered = server_message(&next_payload(&mut returned).await);
    let ServerMessage::AgentMessage { from_agent_id, .. } = delivered else {
        panic!("expected the queued et-agent-message, got {delivered:?}");
    };
    assert_eq!(from_agent_id, sender_id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_binary_relay_sent_as_text_reaches_peers_as_binary() {
    let server = start();
    let (mut sender, _sender_id) = connect_agent(&server.ws_url).await;
    let (mut peer, _peer_id) = connect_agent(&server.ws_url).await;

    send_json(
        &mut sender,
        &serde_json::json!({"type": "et-relay-binary", "content": [1_u8, 2_u8, 3_u8]}),
    )
    .await;

    assert_eq!(
        next_payload(&mut peer).await,
        Message::Binary(Bytes::from_static(&[1, 2, 3]))
    );
}

/// Ask the hub for its roster until it lists `agent_id` as disconnected.
#[expect(
    clippy::single_call_fn,
    reason = "a named wait step of the queued-delivery test, kept out of its body"
)]
async fn wait_until_disconnected(socket: &mut Socket, agent_id: &str) {
    for _ in 0_u32..50 {
        send_json(socket, &serde_json::json!({"type": "et-list-agents"})).await;
        let reply = server_message(&next_text(socket).await);
        if let ServerMessage::ListAgentsResponse { agents } = reply
            && agents
                .iter()
                .any(|agent| agent.agent_id == agent_id && agent.state == AgentConnectionState::Disconnected)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{agent_id} was never listed as disconnected");
}

/// The next text frame on `socket`, skipping control frames.
async fn next_text(socket: &mut Socket) -> Message {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if matches!(frame, Message::Text(_)) {
            return frame;
        }
    }
}

/// The delivery status the hub reports next on `socket`.
#[expect(
    clippy::single_call_fn,
    reason = "a named read step of the queued-delivery test, kept out of its body"
)]
async fn next_status(socket: &mut Socket) -> MessageDeliveryStatus {
    loop {
        if let ServerMessage::MessageStatus { status, .. } = server_message(&next_text(socket).await) {
            return status;
        }
    }
}
