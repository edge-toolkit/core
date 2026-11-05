//! Wire messages across the WIT boundary.
//!
//! The typed `et:ws-messages/messages` values a guest sends and receives are converted to and from
//! `edge_toolkit::ws::{ClientMessage, ServerMessage}`, which every host serialises to JSON for the websocket. Guests
//! no longer hand-craft JSON; whichever transport a host uses, the conversion is the same.

#[cfg(not(feature = "std"))]
use alloc::{
    string::{String, ToString as _},
    vec::Vec,
};
use core::net::IpAddr;

use chrono::{DateTime, Utc};
use edge_toolkit::ws::{
    AgentConnectionState as EtAgentConnectionState, AgentSummary as EtAgentSummary, ClientMessage, CloudEvent,
    ConnectStatus as EtConnectStatus, MessageDeliveryStatus as EtMessageDeliveryStatus, MessageScope as EtMessageScope,
    ServerMessage,
};

use crate::bindings::et::ws_messages::messages::{
    AgentConnectionState as WitAgentConnectionState, AgentMessagePayload, AgentSummary as WitAgentSummary,
    ClientMessage as WitClientMessage, ConnectAckPayload, ConnectStatus as WitConnectStatus, InvalidPayload,
    IpAddress as WitIpAddress, ListAgentsResponsePayload, MessageDeliveryStatus as WitMessageDeliveryStatus,
    MessageScope as WitMessageScope, MessageStatusPayload, RelayBinaryPayload, RelayTextPayload, ResponsePayload,
    ServerMessage as WitServerMessage,
};
use crate::bindings::et::ws_wasi::ws::WsError;

/// One websocket data frame, as whichever transport a host uses carries it.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Frame {
    /// A text frame: an `et-*` message's JSON, or relayed text.
    Text(String),
    /// A binary frame, which is always relayed.
    Binary(Vec<u8>),
}

/// The frame a guest's `client-message` goes out as, which is the frame [`frame_of`] gives its message.
///
/// # Errors
///
/// When the message's opaque JSON or event time does not parse, or the message does not serialise.
pub fn client_frame(message: WitClientMessage) -> Result<Frame, WsError> {
    frame_of(wit_to_client_message(message)?)
}

/// The frame `message` goes out as.
///
/// Relay variants travel as raw text / binary frames (no JSON envelope) -- that's what the ws-server hub forwards
/// verbatim to other agents. Typed variants serialise through standard tagged JSON.
///
/// # Errors
///
/// When the message does not serialise.
pub fn frame_of(message: ClientMessage) -> Result<Frame, WsError> {
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "every non-relay ClientMessage variant takes the standard tagged-JSON path"
    )]
    Ok(match message {
        ClientMessage::RelayText { content } => Frame::Text(content),
        ClientMessage::RelayBinary { content } => Frame::Binary(content),
        typed => Frame::Text(serde_json::to_string(&typed)?),
    })
}

/// The message an inbound frame carries: an `et-*` message, or the frame as a relay.
///
/// # Errors
///
/// When a text frame claims an `et-*` type but its payload does not match it; a host drops such a frame.
pub fn server_message(frame: Frame) -> Result<ServerMessage, serde_path_to_error::Error<serde_json::Error>> {
    match frame {
        Frame::Text(text) => ServerMessage::from_text_frame(&text),
        Frame::Binary(bytes) => Ok(ServerMessage::from_binary_frame(bytes)),
    }
}

/// Convert a guest-emitted WIT `client-message` into the canonical Rust `ClientMessage`.
///
/// Opaque JSON fields (sent as `string` over WIT) are parsed here so the host always works with `serde_json::Value`
/// payloads.
#[expect(clippy::single_call_fn, reason = "named converter; used once by client_frame")]
fn wit_to_client_message(msg: WitClientMessage) -> Result<ClientMessage, WsError> {
    let parse_value = |raw: String| -> Result<serde_json::Value, WsError> {
        let mut deserializer = serde_json::Deserializer::from_str(&raw);
        Ok(serde_path_to_error::deserialize(&mut deserializer)?)
    };
    Ok(match msg {
        WitClientMessage::Connect(payload) => ClientMessage::Connect {
            agent_id: payload.agent_id,
        },
        WitClientMessage::Alive(payload) => ClientMessage::Alive {
            timestamp: payload.timestamp,
        },
        WitClientMessage::ListAgents => ClientMessage::ListAgents,
        WitClientMessage::SendAgentMessage(payload) => ClientMessage::SendAgentMessage {
            to_agent_id: payload.to_agent_id,
            message: parse_value(payload.message)?,
        },
        WitClientMessage::BroadcastMessage(payload) => ClientMessage::BroadcastMessage {
            message: parse_value(payload.message)?,
        },
        WitClientMessage::MessageAck(payload) => ClientMessage::MessageAck {
            message_id: payload.message_id,
        },
        WitClientMessage::ClientEvent(payload) => {
            let event = payload.event;
            let mut wire = CloudEvent::new(
                event.id,
                event.source,
                event.type_,
                DateTime::parse_from_rfc3339(&event.time)?.with_timezone(&Utc),
                parse_value(event.data)?,
            );
            wire.specversion = serde_json::from_value(serde_json::Value::String(event.specversion))?;
            ClientMessage::ClientEvent { event: wire }
        }
        WitClientMessage::RelayText(payload) => ClientMessage::RelayText {
            content: payload.content,
        },
        WitClientMessage::RelayBinary(payload) => ClientMessage::RelayBinary {
            content: payload.content,
        },
    })
}

/// Convert a wire `ServerMessage` (inbound) into the WIT `server-message` the guest sees on `recv`.
///
/// Re-serialises opaque JSON payloads so they cross the WIT boundary as strings.
///
/// # Errors
///
/// Fails when an agent message's opaque JSON payload cannot be re-serialised.
pub fn server_message_to_wit(msg: ServerMessage) -> Result<WitServerMessage, WsError> {
    let serialize = |value: serde_json::Value| serde_json::to_string(&value);
    Ok(match msg {
        ServerMessage::ConnectAck { agent_id, status } => WitServerMessage::ConnectAck(ConnectAckPayload {
            agent_id,
            status: et_connect_status(status),
        }),
        ServerMessage::ListAgentsResponse { agents } => {
            WitServerMessage::ListAgentsResponse(ListAgentsResponsePayload {
                agents: agents.into_iter().map(et_agent_summary).collect(),
            })
        }
        ServerMessage::AgentMessage {
            message_id,
            from_agent_id,
            scope,
            server_received_at,
            message,
        } => WitServerMessage::AgentMessage(AgentMessagePayload {
            message_id,
            from_agent_id,
            scope: et_message_scope(scope),
            server_received_at,
            message: serialize(message)?,
        }),
        ServerMessage::MessageStatus {
            message_id,
            status,
            detail,
        } => WitServerMessage::MessageStatus(MessageStatusPayload {
            message_id,
            status: et_delivery_status(status),
            detail,
        }),
        ServerMessage::Invalid { message_id, detail } => {
            WitServerMessage::Invalid(InvalidPayload { detail, message_id })
        }
        ServerMessage::Response { message } => WitServerMessage::Response(ResponsePayload { message }),
        ServerMessage::RelayText { content } => WitServerMessage::RelayText(RelayTextPayload { content }),
        ServerMessage::RelayBinary { content } => WitServerMessage::RelayBinary(RelayBinaryPayload { content }),
    })
}

// edge-toolkit-to-WIT enum mirrors. Only the `et_*` direction is used: the host never deserialises a WIT-side client
// message back into an edge-toolkit type (that would mean reading something the *server* sent through the *guest's*
// outbound API, which never happens), so the reverse-direction helpers from the pre-split file are gone.
#[expect(
    clippy::needless_pass_by_value,
    clippy::single_call_fn,
    reason = "WIT/edge-toolkit enum bridge; Copy enum, by value keeps call sites uniform"
)]
const fn et_connect_status(value: EtConnectStatus) -> WitConnectStatus {
    match value {
        EtConnectStatus::Assigned => WitConnectStatus::Assigned,
        EtConnectStatus::Reconnected => WitConnectStatus::Reconnected,
    }
}

#[expect(
    clippy::needless_pass_by_value,
    clippy::single_call_fn,
    reason = "WIT/edge-toolkit enum bridge; Copy enum, by value keeps call sites uniform"
)]
const fn et_message_scope(value: EtMessageScope) -> WitMessageScope {
    match value {
        EtMessageScope::Direct => WitMessageScope::Direct,
        EtMessageScope::Broadcast => WitMessageScope::Broadcast,
    }
}

#[expect(
    clippy::needless_pass_by_value,
    clippy::single_call_fn,
    reason = "WIT/edge-toolkit enum bridge; Copy enum, by value keeps call sites uniform"
)]
const fn et_delivery_status(value: EtMessageDeliveryStatus) -> WitMessageDeliveryStatus {
    match value {
        EtMessageDeliveryStatus::Delivered => WitMessageDeliveryStatus::Delivered,
        EtMessageDeliveryStatus::Queued => WitMessageDeliveryStatus::Queued,
        EtMessageDeliveryStatus::Acknowledged => WitMessageDeliveryStatus::Acknowledged,
        EtMessageDeliveryStatus::Broadcast => WitMessageDeliveryStatus::Broadcast,
    }
}

#[expect(
    clippy::needless_pass_by_value,
    clippy::single_call_fn,
    reason = "WIT/edge-toolkit enum bridge; Copy enum, by value keeps call sites uniform"
)]
const fn et_agent_connection_state(value: EtAgentConnectionState) -> WitAgentConnectionState {
    match value {
        EtAgentConnectionState::Connected => WitAgentConnectionState::Connected,
        EtAgentConnectionState::Disconnected => WitAgentConnectionState::Disconnected,
    }
}

#[expect(
    clippy::single_call_fn,
    reason = "summary converter; one call site in server_message_to_wit"
)]
fn et_agent_summary(value: EtAgentSummary) -> WitAgentSummary {
    WitAgentSummary {
        agent_id: value.agent_id,
        state: et_agent_connection_state(value.state),
        last_known_ip: et_ip_address(value.last_known_ip),
    }
}

#[expect(
    clippy::single_call_fn,
    reason = "address converter; one call site in et_agent_summary"
)]
fn et_ip_address(value: IpAddr) -> WitIpAddress {
    match value {
        IpAddr::V4(addr) => WitIpAddress::Ipv4(addr.octets().into()),
        IpAddr::V6(addr) => WitIpAddress::Ipv6(addr.segments().into()),
    }
}

impl From<serde_json::Error> for WsError {
    fn from(err: serde_json::Error) -> Self {
        Self::Decode(err.to_string())
    }
}

impl From<serde_path_to_error::Error<serde_json::Error>> for WsError {
    fn from(err: serde_path_to_error::Error<serde_json::Error>) -> Self {
        Self::Decode(err.to_string())
    }
}

/// A guest event `time` that is not an RFC 3339 timestamp cannot be encoded onto the wire.
impl From<chrono::ParseError> for WsError {
    fn from(err: chrono::ParseError) -> Self {
        Self::Decode(err.to_string())
    }
}
