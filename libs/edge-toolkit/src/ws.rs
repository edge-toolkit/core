use serde::{Deserialize, Serialize};

/// Schema for `serde_json::Value`-typed fields. schemars 1.x renders bare
/// `Value` as the boolean schema `true`, which the `AsyncAPI` Schema model in
/// `asyncapi-rust` 0.2 doesn't accept. Emit an explicit object schema so the
/// payload is described as "arbitrary JSON" without tripping the parser.
#[cfg(feature = "schema-export")]
#[expect(
    clippy::expect_used,
    reason = "static JSON literal -> Schema is infallible; surfacing it loudly if asyncapi-rust ever changes shape"
)]
fn any_json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    serde_json::json!({
        "description": "Arbitrary JSON value (opaque to the protocol)",
    })
    .try_into()
    .expect("any_json_schema is a valid object schema")
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
pub enum WsMessage {
    #[serde(rename = "et-connect")]
    #[cfg_attr(feature = "schema-export", schemars(title = "WsConnect"))]
    Connect { agent_id: Option<String> },
    #[serde(rename = "et-connect-ack")]
    #[cfg_attr(feature = "schema-export", schemars(title = "WsConnectAck"))]
    ConnectAck { agent_id: String, status: ConnectStatus },
    #[serde(rename = "et-alive")]
    #[cfg_attr(feature = "schema-export", schemars(title = "WsAlive"))]
    Alive { timestamp: String },
    #[serde(rename = "et-list-agents")]
    #[cfg_attr(feature = "schema-export", schemars(title = "WsListAgents"))]
    ListAgents,
    #[serde(rename = "et-list-agents-response")]
    #[cfg_attr(feature = "schema-export", schemars(title = "WsListAgentsResponse"))]
    ListAgentsResponse { agents: Vec<AgentSummary> },
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
    #[serde(rename = "et-message-ack")]
    #[cfg_attr(feature = "schema-export", schemars(title = "WsMessageAck"))]
    MessageAck { message_id: String },
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
    #[serde(rename = "et-client-event")]
    #[cfg_attr(feature = "schema-export", schemars(title = "WsClientEvent"))]
    ClientEvent {
        capability: String,
        action: String,
        #[cfg_attr(feature = "schema-export", schemars(schema_with = "any_json_schema"))]
        details: serde_json::Value,
    },
    #[serde(rename = "et-response")]
    #[cfg_attr(feature = "schema-export", schemars(title = "WsResponse"))]
    Response { message: String },
}
