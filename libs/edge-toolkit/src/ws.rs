use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectStatus {
    Assigned,
    Reconnected,
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
    ClientEvent {
        capability: String,
        action: String,
        details: serde_json::Value,
    },
    Response {
        message: String,
    },
}
