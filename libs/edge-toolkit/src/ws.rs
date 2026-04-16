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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsMessage {
    Connect {
        agent_id: Option<String>,
    },
    ConnectAck {
        agent_id: String,
        status: ConnectStatus,
    },
    Alive {
        timestamp: String,
    },
    ListAgents,
    ListAgentsResponse {
        agents: Vec<AgentSummary>,
    },
    SendAgentMessage {
        to_agent_id: String,
        message: serde_json::Value,
    },
    BroadcastMessage {
        message: serde_json::Value,
    },
    AgentMessage {
        message_id: String,
        from_agent_id: String,
        scope: MessageScope,
        server_received_at: String,
        message: serde_json::Value,
    },
    MessageAck {
        message_id: String,
    },
    MessageStatus {
        message_id: Option<String>,
        status: MessageDeliveryStatus,
        detail: String,
    },
    Invalid {
        message_id: Option<String>,
        detail: String,
    },
    ClientEvent {
        capability: String,
        action: String,
        details: serde_json::Value,
    },
    Response {
        message: String,
    },
}
