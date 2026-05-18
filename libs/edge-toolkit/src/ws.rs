use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectStatus {
    Assigned,
    Reconnected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageDeliveryStatus {
    Delivered,
    Queued,
    Acknowledged,
    Broadcast,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageScope {
    Direct,
    Broadcast,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentConnectionState {
    Connected,
    Disconnected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentSummary {
    pub agent_id: String,
    pub state: AgentConnectionState,
    pub last_known_ip: Option<String>,
}

/// Wire protocol envelope for edge-toolkit's WebSocket hub.
///
/// All variants serialise with an `et-` prefix on the `type` tag so frames
/// owned by other schemas (e.g. the split-learning demo's `activations` /
/// `grads` messages) can share the same socket without colliding. Messages
/// that don't match any of these variants are forwarded by the server as a
/// default broadcast — see `et-ws-service`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WsMessage {
    #[serde(rename = "et-connect")]
    Connect {
        agent_id: Option<String>,
    },
    #[serde(rename = "et-connect-ack")]
    ConnectAck {
        agent_id: String,
        status: ConnectStatus,
    },
    #[serde(rename = "et-alive")]
    Alive {
        timestamp: String,
    },
    #[serde(rename = "et-list-agents")]
    ListAgents,
    #[serde(rename = "et-list-agents-response")]
    ListAgentsResponse {
        agents: Vec<AgentSummary>,
    },
    #[serde(rename = "et-send-agent-message")]
    SendAgentMessage {
        to_agent_id: String,
        message: serde_json::Value,
    },
    #[serde(rename = "et-agent-message")]
    AgentMessage {
        message_id: String,
        from_agent_id: String,
        scope: MessageScope,
        server_received_at: String,
        message: serde_json::Value,
    },
    #[serde(rename = "et-message-ack")]
    MessageAck {
        message_id: String,
    },
    #[serde(rename = "et-message-status")]
    MessageStatus {
        message_id: Option<String>,
        status: MessageDeliveryStatus,
        detail: String,
    },
    #[serde(rename = "et-invalid")]
    Invalid {
        message_id: Option<String>,
        detail: String,
    },
    #[serde(rename = "et-client-event")]
    ClientEvent {
        capability: String,
        action: String,
        details: serde_json::Value,
    },
    #[serde(rename = "et-store-file")]
    StoreFile {
        filename: String,
    },
    #[serde(rename = "et-fetch-file")]
    FetchFile {
        agent_id: String,
        filename: String,
    },
    #[serde(rename = "et-response")]
    Response {
        message: String,
    },
}
