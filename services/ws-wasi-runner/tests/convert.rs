//! Covers the frame each guest `client-message` goes out as, the message each inbound frame carries, and the
//! `server-message` each of those reaches the guest as.
#![cfg(test)]

use chrono::{DateTime, Utc};
use edge_toolkit::ws::{
    AgentConnectionState, AgentSummary, ClientMessage, CloudEvent as EtCloudEvent, ConnectStatus,
    MessageDeliveryStatus, MessageScope, ServerMessage,
};
use et_ws_wasi_runner::bindings::et::ws_messages::messages::{
    AgentConnectionState as WitAgentConnectionState, AlivePayload, BroadcastMessagePayload, ClientEventPayload,
    ClientMessage as WitClientMessage, CloudEvent, ConnectPayload, ConnectStatus as WitConnectStatus,
    MessageAckPayload, MessageDeliveryStatus as WitMessageDeliveryStatus, MessageScope as WitMessageScope,
    RelayBinaryPayload, RelayTextPayload, SendAgentMessagePayload, ServerMessage as WitServerMessage,
};
use et_ws_wasi_runner::bindings::et::ws_wasi::ws::WsError;
use et_ws_wasi_runner::convert::{Frame, client_frame, server_message, server_message_to_wit};
use rstest::rstest;
use serde_json::json;

/// The time every event below carries.
const TIME: &str = "2026-11-02T06:30:00Z";

/// The text frame `message` serialises to.
fn typed(message: &ClientMessage) -> Frame {
    Frame::Text(serde_json::to_string(message).unwrap())
}

/// A guest's event, with `time` and `specversion` as given.
fn event(time: &str, specversion: &str) -> WitClientMessage {
    WitClientMessage::ClientEvent(ClientEventPayload {
        event: CloudEvent {
            data: r#"{"reading":1}"#.to_owned(),
            id: "event-1".to_owned(),
            source: "/sensor".to_owned(),
            specversion: specversion.to_owned(),
            time: time.to_owned(),
            type_: "et.reading".to_owned(),
        },
    })
}

#[rstest]
#[case::connect(
    WitClientMessage::Connect(ConnectPayload { agent_id: Some("agent-1".to_owned()) }),
    typed(&ClientMessage::Connect { agent_id: Some("agent-1".to_owned()) }),
)]
#[case::alive(
    WitClientMessage::Alive(AlivePayload { timestamp: TIME.to_owned() }),
    typed(&ClientMessage::Alive { timestamp: TIME.to_owned() }),
)]
#[case::list_agents(WitClientMessage::ListAgents, typed(&ClientMessage::ListAgents))]
#[case::send_agent_message(
    WitClientMessage::SendAgentMessage(SendAgentMessagePayload {
        message: r#"{"n":2}"#.to_owned(),
        to_agent_id: "agent-2".to_owned(),
    }),
    typed(&ClientMessage::SendAgentMessage { to_agent_id: "agent-2".to_owned(), message: json!({ "n": 2_i32 }) }),
)]
#[case::broadcast_message(
    WitClientMessage::BroadcastMessage(BroadcastMessagePayload { message: "[1,2]".to_owned() }),
    typed(&ClientMessage::BroadcastMessage { message: json!([1_i32, 2_i32]) }),
)]
#[case::message_ack(
    WitClientMessage::MessageAck(MessageAckPayload { message_id: "message-1".to_owned() }),
    typed(&ClientMessage::MessageAck { message_id: "message-1".to_owned() }),
)]
#[case::client_event(
    event(TIME, "1.0"),
    typed(&ClientMessage::ClientEvent {
        event: EtCloudEvent::new(
            "event-1".to_owned(),
            "/sensor".to_owned(),
            "et.reading".to_owned(),
            DateTime::parse_from_rfc3339(TIME).unwrap().with_timezone(&Utc),
            json!({ "reading": 1_i32 }),
        ),
    }),
)]
#[case::relay_text(
    WitClientMessage::RelayText(RelayTextPayload { content: "hello".to_owned() }),
    Frame::Text("hello".to_owned()),
)]
#[case::relay_binary(
    WitClientMessage::RelayBinary(RelayBinaryPayload { content: vec![0, 1, 2] }),
    Frame::Binary(vec![0, 1, 2]),
)]
fn each_client_message_goes_out_as_its_frame(#[case] message: WitClientMessage, #[case] expected: Frame) {
    assert_eq!(client_frame(message).unwrap(), expected);
}

#[rstest]
#[case::opaque_json_that_is_not_json(WitClientMessage::BroadcastMessage(BroadcastMessagePayload {
    message: "{".to_owned(),
}))]
#[case::event_time_that_is_not_rfc_3339(event("yesterday", "1.0"))]
#[case::event_specversion_no_hub_knows(event(TIME, "0.3"))]
fn a_client_message_that_cannot_be_encoded_is_a_decode_error(#[case] message: WitClientMessage) {
    assert!(matches!(client_frame(message), Err(WsError::Decode(_))));
}

#[test]
fn a_binary_frame_is_relayed() {
    assert!(matches!(
        server_message(Frame::Binary(vec![7])).unwrap(),
        ServerMessage::RelayBinary { content } if content == [7]
    ));
}

#[test]
fn a_text_frame_claiming_a_type_it_does_not_match_is_rejected() {
    let frame = Frame::Text(r#"{"type":"et-connect-ack"}"#.to_owned());
    let _rejection = server_message(frame).unwrap_err();
}

#[test]
fn a_reconnect_ack_keeps_its_status() {
    let ack = ServerMessage::ConnectAck {
        agent_id: "agent-1".to_owned(),
        status: ConnectStatus::Reconnected,
    };
    assert!(matches!(
        server_message_to_wit(ack).unwrap(),
        WitServerMessage::ConnectAck(payload) if matches!(payload.status, WitConnectStatus::Reconnected)
    ));
}

#[test]
fn a_disconnected_agent_keeps_its_state() {
    let agents = vec![AgentSummary::new(
        "agent-1".to_owned(),
        AgentConnectionState::Disconnected,
        "192.0.2.1".parse().unwrap(),
    )];
    let WitServerMessage::ListAgentsResponse(payload) =
        server_message_to_wit(ServerMessage::ListAgentsResponse { agents }).unwrap()
    else {
        panic!("a list-agents response must stay a list-agents response");
    };
    assert!(matches!(
        payload.agents.as_slice(),
        [agent] if matches!(agent.state, WitAgentConnectionState::Disconnected)
    ));
}

#[rstest]
#[case::direct(MessageScope::Direct, WitMessageScope::Direct)]
#[case::broadcast(MessageScope::Broadcast, WitMessageScope::Broadcast)]
fn an_agent_message_carries_its_scope_and_payload(#[case] scope: MessageScope, #[case] expected: WitMessageScope) {
    let message = ServerMessage::AgentMessage {
        message_id: "message-1".to_owned(),
        from_agent_id: "agent-2".to_owned(),
        scope,
        server_received_at: TIME.to_owned(),
        message: json!({ "n": 2_i32 }),
    };
    let WitServerMessage::AgentMessage(payload) = server_message_to_wit(message).unwrap() else {
        panic!("an agent message must stay an agent message");
    };
    assert_eq!(payload.scope, expected);
    assert_eq!(payload.message, r#"{"n":2}"#);
    assert_eq!(
        (
            payload.message_id.as_str(),
            payload.from_agent_id.as_str(),
            payload.server_received_at.as_str()
        ),
        ("message-1", "agent-2", TIME)
    );
}

#[rstest]
#[case::delivered(MessageDeliveryStatus::Delivered, WitMessageDeliveryStatus::Delivered)]
#[case::queued(MessageDeliveryStatus::Queued, WitMessageDeliveryStatus::Queued)]
#[case::acknowledged(MessageDeliveryStatus::Acknowledged, WitMessageDeliveryStatus::Acknowledged)]
#[case::broadcast(MessageDeliveryStatus::Broadcast, WitMessageDeliveryStatus::Broadcast)]
fn a_message_status_carries_its_status(
    #[case] status: MessageDeliveryStatus,
    #[case] expected: WitMessageDeliveryStatus,
) {
    let message = ServerMessage::MessageStatus {
        message_id: Some("message-1".to_owned()),
        status,
        detail: "ok".to_owned(),
    };
    let WitServerMessage::MessageStatus(payload) = server_message_to_wit(message).unwrap() else {
        panic!("a message status must stay a message status");
    };
    assert_eq!(payload.status, expected);
    assert_eq!(
        (payload.message_id.as_deref(), payload.detail.as_str()),
        (Some("message-1"), "ok")
    );
}

#[test]
fn an_invalid_notice_carries_its_detail() {
    let message = ServerMessage::Invalid {
        message_id: None,
        detail: "unknown type".to_owned(),
    };
    assert!(matches!(
        server_message_to_wit(message).unwrap(),
        WitServerMessage::Invalid(payload) if payload.message_id.is_none() && payload.detail == "unknown type"
    ));
}

#[test]
fn a_response_carries_its_message() {
    let message = ServerMessage::Response {
        message: "pong".to_owned(),
    };
    assert!(matches!(
        server_message_to_wit(message).unwrap(),
        WitServerMessage::Response(payload) if payload.message == "pong"
    ));
}

#[rstest]
#[case::text(
    ServerMessage::RelayText { content: "hello".to_owned() },
    |message: &WitServerMessage| matches!(message, WitServerMessage::RelayText(payload) if payload.content == "hello"),
)]
#[case::binary(
    ServerMessage::RelayBinary { content: vec![7] },
    |message: &WitServerMessage| matches!(message, WitServerMessage::RelayBinary(payload) if payload.content == [7]),
)]
fn a_relayed_frame_reaches_the_guest_unchanged(
    #[case] message: ServerMessage,
    #[case] expected: fn(&WitServerMessage) -> bool,
) {
    assert!(expected(&server_message_to_wit(message).unwrap()));
}
