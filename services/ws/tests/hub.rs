//! The `no_std` routing core, `hub::Link`, driven directly with no transport: each frame's replies, deliveries and
//! shared steps. `S = String` stands in for the session handle, named after the agent that holds it.
#![cfg(test)]

use std::net::{IpAddr, Ipv4Addr};

use bytes::Bytes;
use edge_toolkit::ws::{ClientMessage, ConnectStatus, MessageDeliveryStatus, MessageScope, ServerMessage};
use edge_toolkit::ws_server::{AgentRegistry, MessageRetention};
use et_ws_service::SessionMessage;
use et_ws_service::hub::{Link, Routing, Shared, Stamps};
use serde_json::json;

const CLIENT_IP: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const NOW: &str = "2026-10-29T00:00:00Z";

/// Ids `id-1`, `id-2`, ... in turn, and a fixed time.
#[derive(Default)]
struct Counter(u32);

impl Stamps for Counter {
    fn new_id(&mut self) -> String {
        self.0 = self.0.saturating_add(1);
        format!("id-{}", self.0)
    }

    fn now(&mut self) -> String {
        NOW.to_owned()
    }
}

/// A link that has connected as `agent_id`, its session named after it.
fn connected(registry: &AgentRegistry<String>, agent_id: &str, stamps: &mut Counter) -> Link<String> {
    let mut link = Link::new(CLIENT_IP, agent_id.to_owned(), MessageRetention::All);
    let connect = ClientMessage::Connect {
        agent_id: Some(agent_id.to_owned()),
    };
    let _routing = link.route(registry, connect, stamps);
    link
}

/// The one reply `routing` holds.
fn only_reply(routing: Routing<String>) -> ServerMessage {
    let [reply] = <[ServerMessage; 1]>::try_from(routing.replies).unwrap();
    reply
}

/// The status and detail of a `MessageStatus` reply.
fn status_of(reply: &ServerMessage) -> (&MessageDeliveryStatus, &str) {
    let ServerMessage::MessageStatus { status, detail, .. } = reply else {
        panic!("not a status: {reply:?}");
    };
    (status, detail)
}

/// The detail of an `Invalid` reply.
fn refusal_of(reply: &ServerMessage) -> &str {
    let ServerMessage::Invalid { detail, .. } = reply else {
        panic!("not a refusal: {reply:?}");
    };
    detail
}

#[test]
fn a_connect_takes_an_id_acks_it_and_joins() {
    let registry = AgentRegistry::<String>::default();
    let mut stamps = Counter::default();
    let mut link = Link::new(CLIENT_IP, "s".to_owned(), MessageRetention::LatestPerSender);
    assert_eq!(link.current_agent_id(), "unassigned");
    assert_eq!((link.client_ip(), link.session().as_str()), (CLIENT_IP, "s"));
    assert_eq!(link.retention(), MessageRetention::LatestPerSender);

    let routing = link.route(&registry, ClientMessage::Connect { agent_id: None }, &mut stamps);
    assert_eq!(link.agent_id(), Some("id-1"));
    assert!(routing.deliveries.is_empty());
    assert!(matches!(routing.shared.as_slice(), [Shared::Joined { agent_id }] if agent_id == "id-1"));
    assert!(matches!(
        only_reply(routing),
        ServerMessage::ConnectAck { agent_id, status: ConnectStatus::Assigned } if agent_id == "id-1"
    ));
}

#[test]
fn a_reconnect_receives_what_was_queued_while_it_was_away() {
    let registry = AgentRegistry::<String>::default();
    let mut stamps = Counter::default();
    let away = connected(&registry, "a", &mut stamps);
    let mut sender = connected(&registry, "b", &mut stamps);
    assert!(away.release(&registry, &|session| session == "a"));

    let send = ClientMessage::SendAgentMessage {
        to_agent_id: "a".to_owned(),
        message: json!({"n": 1_i32}),
    };
    let queued = sender.route(&registry, send, &mut stamps);
    assert!(queued.deliveries.is_empty());
    assert_eq!(
        status_of(&only_reply(queued)),
        (&MessageDeliveryStatus::Queued, "message queued for agent a")
    );

    let mut back = Link::new(CLIENT_IP, "a2".to_owned(), MessageRetention::All);
    let connect = ClientMessage::Connect {
        agent_id: Some("a".to_owned()),
    };
    let routing = back.route(&registry, connect, &mut stamps);
    let [ack, pending] = <[ServerMessage; 2]>::try_from(routing.replies).unwrap();
    assert!(matches!(
        ack,
        ServerMessage::ConnectAck {
            status: ConnectStatus::Reconnected,
            ..
        }
    ));
    assert!(matches!(
        pending,
        ServerMessage::AgentMessage { from_agent_id, scope: MessageScope::Direct, server_received_at, .. }
            if from_agent_id == "b" && server_received_at == NOW
    ));
}

#[test]
fn alive_and_list_are_answered() {
    let registry = AgentRegistry::<String>::default();
    let mut stamps = Counter::default();
    let mut link = connected(&registry, "a", &mut stamps);

    let alive = link.route(
        &registry,
        ClientMessage::Alive {
            timestamp: NOW.to_owned(),
        },
        &mut stamps,
    );
    assert!(matches!(
        only_reply(alive),
        ServerMessage::Response { message } if message == format!("Alive message received at {NOW}")
    ));

    let listed = link.route(&registry, ClientMessage::ListAgents, &mut stamps);
    assert!(matches!(
        only_reply(listed),
        ServerMessage::ListAgentsResponse { agents } if agents.len() == 1 && agents[0].agent_id == "a"
    ));
}

#[test]
fn a_direct_message_is_delivered_to_a_connected_recipient() {
    let registry = AgentRegistry::<String>::default();
    let mut stamps = Counter::default();
    let mut sender = connected(&registry, "a", &mut stamps);
    let _recipient = connected(&registry, "b", &mut stamps);

    let send = ClientMessage::SendAgentMessage {
        to_agent_id: "b".to_owned(),
        message: json!("hi"),
    };
    let routing = sender.route(&registry, send, &mut stamps);
    assert!(routing.shared.is_empty());
    assert!(matches!(
        routing.deliveries.as_slice(),
        [(session, SessionMessage::Json(ServerMessage::AgentMessage { from_agent_id, .. }))]
            if session == "b" && from_agent_id == "a"
    ));
    assert_eq!(
        status_of(&only_reply(routing)),
        (&MessageDeliveryStatus::Delivered, "message delivered to agent b")
    );
}

#[test]
fn a_direct_message_is_refused_unconnected_to_itself_and_to_a_stranger() {
    let registry = AgentRegistry::<String>::default();
    let mut stamps = Counter::default();
    let send = |recipient: &str| ClientMessage::SendAgentMessage {
        to_agent_id: recipient.to_owned(),
        message: json!(null),
    };

    let mut unconnected = Link::new(CLIENT_IP, "u".to_owned(), MessageRetention::All);
    let refused = unconnected.route(&registry, send("a"), &mut stamps);
    assert_eq!(
        refusal_of(&only_reply(refused)),
        "agent must connect before sending messages"
    );

    let mut link = connected(&registry, "a", &mut stamps);
    let to_itself = link.route(&registry, send("a"), &mut stamps);
    assert_eq!(
        refusal_of(&only_reply(to_itself)),
        "agent cannot send a direct message to itself"
    );
    let to_stranger = link.route(&registry, send("nobody"), &mut stamps);
    assert_eq!(refusal_of(&only_reply(to_stranger)), "unknown target agent nobody");
}

#[test]
fn a_broadcast_reaches_every_other_agent_and_every_other_hub() {
    let registry = AgentRegistry::<String>::default();
    let mut stamps = Counter::default();
    let mut sender = connected(&registry, "a", &mut stamps);
    let _other = connected(&registry, "b", &mut stamps);

    let broadcast = ClientMessage::BroadcastMessage { message: json!(1_i32) };
    let routing = sender.route(&registry, broadcast, &mut stamps);
    assert!(matches!(
        routing.deliveries.as_slice(),
        [(session, SessionMessage::Json(ServerMessage::AgentMessage { scope: MessageScope::Broadcast, .. }))]
            if session == "b"
    ));
    assert!(matches!(
        routing.shared.as_slice(),
        [Shared::Broadcast { from_agent_id, frame: SessionMessage::Json(_) }] if from_agent_id == "a"
    ));
    assert_eq!(
        status_of(&only_reply(routing)),
        (
            &MessageDeliveryStatus::Broadcast,
            "broadcast sent to 1 connected agents"
        )
    );

    let mut unconnected = Link::new(CLIENT_IP, "u".to_owned(), MessageRetention::All);
    let refused = unconnected.route(
        &registry,
        ClientMessage::BroadcastMessage { message: json!(1_i32) },
        &mut stamps,
    );
    assert_eq!(
        refusal_of(&only_reply(refused)),
        "agent must connect before broadcasting messages"
    );
}

#[test]
fn an_acknowledgement_tells_both_ends() {
    let registry = AgentRegistry::<String>::default();
    let mut stamps = Counter::default();
    let mut sender = connected(&registry, "a", &mut stamps);
    let mut recipient = connected(&registry, "b", &mut stamps);
    let direct = ClientMessage::SendAgentMessage {
        to_agent_id: "b".to_owned(),
        message: json!(null),
    };
    let delivered = sender.route(&registry, direct, &mut stamps);
    let ServerMessage::MessageStatus {
        message_id: Some(message_id),
        ..
    } = only_reply(delivered)
    else {
        panic!("the send was not given a message id");
    };

    let ack = ClientMessage::MessageAck {
        message_id: message_id.clone(),
    };
    let routing = recipient.route(&registry, ack, &mut stamps);
    assert!(matches!(
        routing.deliveries.as_slice(),
        [(session, SessionMessage::Json(ServerMessage::MessageStatus { detail, .. }))]
            if session == "a" && detail == "agent b acknowledged receipt"
    ));
    assert_eq!(
        status_of(&only_reply(routing)),
        (&MessageDeliveryStatus::Acknowledged, "message acknowledged")
    );

    let again = recipient.route(&registry, ClientMessage::MessageAck { message_id }, &mut stamps);
    assert_eq!(refusal_of(&only_reply(again)), "no pending message to acknowledge");

    let mut unconnected = Link::new(CLIENT_IP, "u".to_owned(), MessageRetention::All);
    let ack = ClientMessage::MessageAck {
        message_id: "m".to_owned(),
    };
    let refused = unconnected.route(&registry, ack, &mut stamps);
    assert_eq!(
        refusal_of(&only_reply(refused)),
        "agent must connect before acknowledging messages"
    );
}

#[test]
fn a_client_event_is_only_logged() {
    let registry = AgentRegistry::<String>::default();
    let mut stamps = Counter::default();
    let mut link = connected(&registry, "a", &mut stamps);
    for event_type in ["et.video_cv.inference", "et.sensor.reading"] {
        let frame = json!({
            "type": "et-client-event",
            "event": {
                "data": {"detected_class": "cat", "confidence": 0.5_f64, "processed_at": NOW},
                "type": event_type,
                "id": "e1",
                "source": "/modules/x",
                "specversion": "1.0",
                "time": NOW,
            },
        });
        let event = ClientMessage::from_text_frame(&frame.to_string()).unwrap();
        let routing = link.route(&registry, event, &mut stamps);
        assert!(routing.deliveries.is_empty() && routing.shared.is_empty() && routing.replies.is_empty());
    }
}

#[test]
fn a_relay_registers_its_sender_and_reaches_every_other_agent() {
    let registry = AgentRegistry::<String>::default();
    let mut stamps = Counter::default();
    let _listener = connected(&registry, "a", &mut stamps);
    let mut relay = Link::new(CLIENT_IP, "r".to_owned(), MessageRetention::All);

    let text = relay.route(
        &registry,
        ClientMessage::RelayText {
            content: "raw".to_owned(),
        },
        &mut stamps,
    );
    let relay_id = relay.agent_id().unwrap().to_owned();
    assert!(matches!(
        text.deliveries.as_slice(),
        [(session, SessionMessage::Text(content))] if session == "a" && content == "raw"
    ));
    assert!(matches!(
        text.shared.as_slice(),
        [Shared::Joined { agent_id }, Shared::Broadcast { from_agent_id, .. }]
            if *agent_id == relay_id && *from_agent_id == relay_id
    ));
    assert!(text.replies.is_empty());
}

#[test]
fn a_binary_relay_reaches_every_other_agent_as_binary() {
    let registry = AgentRegistry::<String>::default();
    let mut stamps = Counter::default();
    let _listener = connected(&registry, "a", &mut stamps);
    let mut relay = Link::new(CLIENT_IP, "r".to_owned(), MessageRetention::All);

    // `et-relay-binary` sent as a text frame is relayed as a binary one.
    let binary = relay.route(
        &registry,
        ClientMessage::RelayBinary { content: vec![1, 2] },
        &mut stamps,
    );
    assert!(matches!(
        binary.deliveries.as_slice(),
        [(session, SessionMessage::Binary(bytes))] if session == "a" && bytes.as_ref() == [1, 2]
    ));

    // Registered by that frame, so the raw binary frame after it joins nothing.
    let raw = relay.route_binary(&registry, Bytes::from_static(&[3]), &mut stamps);
    assert!(matches!(
        raw.deliveries.as_slice(),
        [(_, SessionMessage::Binary(bytes))] if bytes.as_ref() == [3]
    ));
    assert!(matches!(raw.shared.as_slice(), [Shared::Broadcast { .. }]));
}

#[test]
fn a_displaced_link_cannot_release_its_replacement() {
    let registry = AgentRegistry::<String>::default();
    let mut stamps = Counter::default();
    let first = connected(&registry, "a", &mut stamps);
    let mut second = Link::new(CLIENT_IP, "a-again".to_owned(), MessageRetention::All);
    let connect = ClientMessage::Connect {
        agent_id: Some("a".to_owned()),
    };
    let _routing = second.route(&registry, connect, &mut stamps);

    assert!(!first.release(&registry, &|session| session == first.session()));
    assert!(second.release(&registry, &|session| session == second.session()));
    assert!(!Link::new(CLIENT_IP, "never".to_owned(), MessageRetention::All).release(&registry, &|_session| true));
}

#[test]
fn a_server_message_is_sent_as_json() {
    let message = ServerMessage::Response {
        message: "ok".to_owned(),
    };
    assert!(matches!(
        SessionMessage::from(message),
        SessionMessage::Json(ServerMessage::Response { .. })
    ));
}
