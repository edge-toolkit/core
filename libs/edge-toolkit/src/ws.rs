use serde::{Deserialize, Serialize};

/// Schema for `serde_json::Value`-typed fields. schemars 1.x renders bare
/// `Value` as the boolean schema `true`, which the `AsyncAPI` Schema model in
/// `asyncapi-rust` 0.2 doesn't accept. Emit an explicit object schema so the
/// payload is described as "arbitrary JSON" without tripping the parser.
#[cfg(feature = "schema-export")]
#[expect(
    clippy::unwrap_used,
    reason = "static JSON literal -> Schema conversion is infallible by construction"
)]
fn any_json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    serde_json::json!({
        "description": "Arbitrary JSON value (opaque to the protocol)",
    })
    .try_into()
    .unwrap()
}

/// Schema for `Vec<u8>` byte-array fields. schemars 1.x's default `Vec<u8>`
/// rendering is `{"type":"array","items":{"type":"integer"}}` with no width
/// hint, which int-gen's WIT emitter widens to `list<s64>`. Stamping
/// `format: "uint8"` on the items lets the emitter pick the right
/// `list<u8>` representation.
#[cfg(feature = "schema-export")]
#[expect(
    clippy::unwrap_used,
    reason = "static JSON literal -> Schema conversion is infallible by construction"
)]
fn byte_array_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    serde_json::json!({
        "type": "array",
        "items": { "type": "integer", "format": "uint8", "minimum": 0, "maximum": 255 },
        "description": "Byte array (uint8)",
    })
    .try_into()
    .unwrap()
}

#[expect(
    clippy::exhaustive_enums,
    reason = "wire protocol enum: variants exhaustively describe the JSON shape, downstream matches are exhaustive"
)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema-export", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ConnectStatus {
    Assigned,
    Reconnected,
}

#[expect(
    clippy::exhaustive_enums,
    reason = "wire protocol enum: variants exhaustively describe the JSON shape, downstream matches are exhaustive"
)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema-export", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum MessageDeliveryStatus {
    Delivered,
    Queued,
    Acknowledged,
    Broadcast,
}

#[expect(
    clippy::exhaustive_enums,
    reason = "wire protocol enum: variants exhaustively describe the JSON shape, downstream matches are exhaustive"
)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema-export", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum MessageScope {
    Direct,
    Broadcast,
}

#[expect(
    clippy::exhaustive_enums,
    reason = "wire protocol enum: variants exhaustively describe the JSON shape, downstream matches are exhaustive"
)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema-export", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum AgentConnectionState {
    Connected,
    Disconnected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
#[cfg_attr(feature = "schema-export", derive(schemars::JsonSchema))]
pub struct AgentSummary {
    pub agent_id: String,
    pub state: AgentConnectionState,
    pub last_known_ip: Option<String>,
}

impl AgentSummary {
    #[must_use]
    pub const fn new(agent_id: String, state: AgentConnectionState, last_known_ip: Option<String>) -> Self {
        Self {
            agent_id,
            state,
            last_known_ip,
        }
    }
}

/// Returns `true` if `text` decodes as a JSON object whose `type` field is
/// a string starting with `et-`. The shared gate used by
/// [`ClientMessage::from_text_frame`] and [`ServerMessage::from_text_frame`]
/// to decide whether to parse strictly (our schema) or relay raw (foreign).
fn has_et_prefix(text: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .as_ref()
        .and_then(|value| value.get("type"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|tag| tag.starts_with("et-"))
}

/// Messages a client is allowed to SEND to the server.
///
/// Split from [`ServerMessage`] so the type system rejects client code
/// constructing a `ConnectAck`, and so the server's inbound match arms
/// can be exhaustive without an "unexpected server-originated message"
/// trap.
#[expect(
    clippy::exhaustive_enums,
    reason = "wire protocol enum: variants exhaustively describe the JSON shape, downstream matches are exhaustive"
)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(
    feature = "schema-export",
    derive(schemars::JsonSchema, asyncapi_rust::ToAsyncApiMessage)
)]
#[serde(tag = "type")]
pub enum ClientMessage {
    #[serde(rename = "et-connect")]
    #[cfg_attr(feature = "schema-export", schemars(title = "WsConnect"))]
    Connect { agent_id: Option<String> },
    #[serde(rename = "et-alive")]
    #[cfg_attr(feature = "schema-export", schemars(title = "WsAlive"))]
    Alive { timestamp: String },
    #[serde(rename = "et-list-agents")]
    #[cfg_attr(feature = "schema-export", schemars(title = "WsListAgents"))]
    ListAgents,
    #[serde(rename = "et-send-agent-message")]
    #[cfg_attr(feature = "schema-export", schemars(title = "WsSendAgentMessage"))]
    SendAgentMessage {
        to_agent_id: String,
        #[cfg_attr(feature = "schema-export", schemars(schema_with = "any_json_schema"))]
        message: serde_json::Value,
    },
    #[serde(rename = "et-broadcast-message")]
    #[cfg_attr(feature = "schema-export", schemars(title = "WsBroadcastMessage"))]
    BroadcastMessage {
        #[cfg_attr(feature = "schema-export", schemars(schema_with = "any_json_schema"))]
        message: serde_json::Value,
    },
    #[serde(rename = "et-message-ack")]
    #[cfg_attr(feature = "schema-export", schemars(title = "WsMessageAck"))]
    MessageAck { message_id: String },
    #[serde(rename = "et-client-event")]
    #[cfg_attr(feature = "schema-export", schemars(title = "WsClientEvent"))]
    ClientEvent {
        capability: String,
        action: String,
        #[cfg_attr(feature = "schema-export", schemars(schema_with = "any_json_schema"))]
        details: serde_json::Value,
    },
    // Foreign frames the server forwards verbatim via its hub-relay path.
    // Allowed in both directions: a client can also explicitly send a
    // relay frame for a peer to receive (the server passes it through
    // unchanged). Wire convention: the JSON envelope is bypassed --
    // `from_text_frame` constructs this when no et- tag matches, and the
    // host's `send` emits the raw `content` as the WebSocket frame.
    #[serde(rename = "et-relay-text")]
    #[cfg_attr(feature = "schema-export", schemars(title = "WsClientRelayText"))]
    RelayText { content: String },
    #[serde(rename = "et-relay-binary")]
    #[cfg_attr(feature = "schema-export", schemars(title = "WsClientRelayBinary"))]
    RelayBinary {
        #[cfg_attr(feature = "schema-export", schemars(schema_with = "byte_array_schema"))]
        content: Vec<u8>,
    },
}

impl ClientMessage {
    /// Decode an inbound text WebSocket frame as a `ClientMessage`. If
    /// the JSON's `type` field starts with `et-`, parse strictly via the
    /// tagged catalog -- failure surfaces as `Err`. Otherwise (non-JSON,
    /// no `type`, or non-et `type`) the frame is wrapped in
    /// `ClientMessage::RelayText { content }` for the hub-relay path.
    pub fn from_text_frame(text: &str) -> Result<Self, serde_path_to_error::Error<serde_json::Error>> {
        if has_et_prefix(text) {
            let mut deserializer = serde_json::Deserializer::from_str(text);
            serde_path_to_error::deserialize(&mut deserializer)
        } else {
            Ok(Self::RelayText {
                content: text.to_owned(),
            })
        }
    }

    /// Wrap an inbound binary WebSocket frame as a `ClientMessage`.
    /// Binary frames are always relays; the typed catalog is JSON-only.
    #[must_use]
    pub const fn from_binary_frame(bytes: Vec<u8>) -> Self {
        Self::RelayBinary { content: bytes }
    }
}

/// Messages the server is allowed to SEND to a client (and clients
/// receive). The mirror of [`ClientMessage`]; see that type's doc for
/// the rationale of the split.
#[expect(
    clippy::exhaustive_enums,
    reason = "wire protocol enum: variants exhaustively describe the JSON shape, downstream matches are exhaustive"
)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(
    feature = "schema-export",
    derive(schemars::JsonSchema, asyncapi_rust::ToAsyncApiMessage)
)]
#[serde(tag = "type")]
pub enum ServerMessage {
    #[serde(rename = "et-connect-ack")]
    #[cfg_attr(feature = "schema-export", schemars(title = "WsConnectAck"))]
    ConnectAck { agent_id: String, status: ConnectStatus },
    #[serde(rename = "et-list-agents-response")]
    #[cfg_attr(feature = "schema-export", schemars(title = "WsListAgentsResponse"))]
    ListAgentsResponse { agents: Vec<AgentSummary> },
    #[serde(rename = "et-agent-message")]
    #[cfg_attr(feature = "schema-export", schemars(title = "WsAgentMessage"))]
    AgentMessage {
        message_id: String,
        from_agent_id: String,
        scope: MessageScope,
        server_received_at: String,
        #[cfg_attr(feature = "schema-export", schemars(schema_with = "any_json_schema"))]
        message: serde_json::Value,
    },
    #[serde(rename = "et-message-status")]
    #[cfg_attr(feature = "schema-export", schemars(title = "WsMessageStatus"))]
    MessageStatus {
        message_id: Option<String>,
        status: MessageDeliveryStatus,
        detail: String,
    },
    #[serde(rename = "et-invalid")]
    #[cfg_attr(feature = "schema-export", schemars(title = "WsInvalid"))]
    Invalid { message_id: Option<String>, detail: String },
    #[serde(rename = "et-response")]
    #[cfg_attr(feature = "schema-export", schemars(title = "WsResponse"))]
    Response { message: String },
    #[serde(rename = "et-relay-text")]
    #[cfg_attr(feature = "schema-export", schemars(title = "WsServerRelayText"))]
    RelayText { content: String },
    #[serde(rename = "et-relay-binary")]
    #[cfg_attr(feature = "schema-export", schemars(title = "WsServerRelayBinary"))]
    RelayBinary {
        #[cfg_attr(feature = "schema-export", schemars(schema_with = "byte_array_schema"))]
        content: Vec<u8>,
    },
}

impl ServerMessage {
    /// Decode an inbound text WebSocket frame as a `ServerMessage` (the
    /// thing a client sees coming from the server). Same gate as
    /// [`ClientMessage::from_text_frame`]: strict et- parse, otherwise
    /// wrap in `RelayText`.
    pub fn from_text_frame(text: &str) -> Result<Self, serde_path_to_error::Error<serde_json::Error>> {
        if has_et_prefix(text) {
            let mut deserializer = serde_json::Deserializer::from_str(text);
            serde_path_to_error::deserialize(&mut deserializer)
        } else {
            Ok(Self::RelayText {
                content: text.to_owned(),
            })
        }
    }

    /// Wrap an inbound binary WebSocket frame as a `ServerMessage`.
    #[must_use]
    pub const fn from_binary_frame(bytes: Vec<u8>) -> Self {
        Self::RelayBinary { content: bytes }
    }
}
