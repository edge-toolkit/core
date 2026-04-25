use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_default::DefaultFromSerde;
use serde_inline_default::serde_inline_default;

use crate::config::{OtlpConfig, default_modules_folders};
use crate::ws::{AgentConnectionState, AgentSummary, ConnectStatus};

/// Default storage directory.
#[must_use]
pub fn default_storage_folder() -> PathBuf {
    let project_root = crate::config::get_project_root();
    project_root.join("services/ws-server/storage")
}

/// Modules config.
#[serde_inline_default]
#[derive(Clone, Debug, DefaultFromSerde, Deserialize)]
pub struct ModulesConfig {
    #[serde(default = "default_modules_folders")]
    pub paths: Vec<PathBuf>,
    #[serde_inline_default(String::from("et-ws-server-static"))]
    pub root: String,
}

/// Storage config.
#[derive(Clone, Debug, DefaultFromSerde, Deserialize)]
pub struct StorageConfig {
    #[serde(default = "default_storage_folder")]
    pub path: PathBuf,
}

/// Application config shared across ws-server services.
#[derive(Clone, Debug, DefaultFromSerde, Deserialize)]
pub struct Config {
    /// OpenTelemetry config.
    #[serde(default)]
    pub otlp: Option<OtlpConfig>,
    /// Modules config.
    #[serde(default)]
    pub modules: ModulesConfig,
    /// Storage config.
    #[serde(default)]
    pub storage: StorageConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingDirectMessage {
    pub message_id: String,
    pub from_agent_id: String,
    pub server_received_at: String,
    pub message: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRecord<S> {
    pub state: AgentConnectionState,
    pub last_known_ip: Option<String>,
    #[serde(skip)]
    pub session: Option<S>,
    #[serde(default)]
    pub pending_direct_messages: BTreeMap<String, PendingDirectMessage>,
}

#[derive(Clone)]
pub struct AgentRegistry<S> {
    pub agents: Arc<Mutex<BTreeMap<String, AgentRecord<S>>>>,
}

impl<S> Default for AgentRegistry<S> {
    fn default() -> Self {
        Self {
            agents: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }
}

impl<S: Clone + Default + Send + 'static> AgentRegistry<S> {
    pub fn load(path: &std::path::Path) -> std::io::Result<Self> {
        if !path.exists() {
            log::warn!("Registry file {:?} does not exist, starting with empty registry", path);
            return Ok(Self::default());
        }
        let yaml = std::fs::read_to_string(path)?;
        let agents: BTreeMap<String, AgentRecord<S>> = serde_yaml::from_str(&yaml).map_err(std::io::Error::other)?;
        log::info!("Loaded {} agents from registry {:?}", agents.len(), path);
        Ok(Self {
            agents: Arc::new(Mutex::new(agents)),
        })
    }
}

impl<S: Clone + Send + 'static> AgentRegistry<S> {
    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        let agents = self.agents.lock().expect("agent registry lock poisoned");
        let yaml = serde_yaml::to_string(&*agents).map_err(std::io::Error::other)?;
        std::fs::write(path, yaml)?;
        log::info!("Agent registry saved to {:?}", path);
        Ok(())
    }

    pub fn connect_agent(
        &self,
        requested_id: Option<String>,
        new_id: String,
        client_ip: &str,
        session: S,
    ) -> (String, ConnectStatus) {
        let mut agents = self.agents.lock().expect("agent registry lock poisoned");

        if let Some(requested_id) = requested_id
            && let Some(record) = agents.get_mut(&requested_id)
        {
            record.state = AgentConnectionState::Connected;
            record.last_known_ip = Some(client_ip.to_string());
            record.session = Some(session);
            return (requested_id, ConnectStatus::Reconnected);
        }

        agents.insert(
            new_id.clone(),
            AgentRecord {
                state: AgentConnectionState::Connected,
                last_known_ip: Some(client_ip.to_string()),
                session: Some(session),
                pending_direct_messages: BTreeMap::new(),
            },
        );
        (new_id, ConnectStatus::Assigned)
    }

    pub fn mark_disconnected(&self, agent_id: &str) {
        let mut agents = self.agents.lock().expect("agent registry lock poisoned");
        if let Some(record) = agents.get_mut(agent_id) {
            record.state = AgentConnectionState::Disconnected;
            record.session = None;
        }
    }

    pub fn list_agents(&self) -> Vec<AgentSummary> {
        let agents = self.agents.lock().expect("agent registry lock poisoned");
        let mut summaries = agents
            .iter()
            .map(|(agent_id, record)| AgentSummary {
                agent_id: agent_id.clone(),
                state: record.state.clone(),
                last_known_ip: record.last_known_ip.clone(),
            })
            .collect::<Vec<_>>();
        summaries.sort_by(|left, right| left.agent_id.cmp(&right.agent_id));
        summaries
    }

    pub fn queue_direct(
        &self,
        message_id: String,
        from_agent_id: &str,
        to_agent_id: &str,
        server_received_at: String,
        message: serde_json::Value,
    ) -> (PendingDirectMessage, Option<S>) {
        let mut agents = self.agents.lock().expect("agent registry lock poisoned");
        let recipient = agents
            .get_mut(to_agent_id)
            .expect("queue_direct called for unknown target agent");

        let pending = PendingDirectMessage {
            message_id,
            from_agent_id: from_agent_id.to_string(),
            server_received_at,
            message,
        };
        recipient
            .pending_direct_messages
            .insert(from_agent_id.to_string(), pending);

        let session = recipient.session.clone();
        let pending = recipient
            .pending_direct_messages
            .get(from_agent_id)
            .expect("pending direct message was just inserted")
            .clone();
        (pending, session)
    }

    pub fn pending_messages_for(&self, agent_id: &str) -> Vec<PendingDirectMessage> {
        let agents = self.agents.lock().expect("agent registry lock poisoned");
        agents
            .get(agent_id)
            .map(|record| {
                let mut pending = record.pending_direct_messages.values().cloned().collect::<Vec<_>>();
                pending.sort_by(|left, right| left.message_id.cmp(&right.message_id));
                pending
            })
            .unwrap_or_default()
    }

    /// Returns `(message_id, sender_session, sender_agent_id)` on success, or an error detail string.
    pub fn acknowledge_message(
        &self,
        recipient_agent_id: &str,
        message_id: &str,
    ) -> Result<(String, Option<S>, String), String> {
        let mut agents = self.agents.lock().expect("agent registry lock poisoned");
        let Some(recipient) = agents.get_mut(recipient_agent_id) else {
            return Err(format!("unknown acknowledging agent {}", recipient_agent_id));
        };

        let Some(sender_agent_id) = recipient
            .pending_direct_messages
            .iter()
            .find_map(|(id, p)| (p.message_id == message_id).then(|| id.clone()))
        else {
            return Err("no pending message to acknowledge".to_string());
        };

        let pending = recipient
            .pending_direct_messages
            .remove(&sender_agent_id)
            .expect("pending direct message disappeared during acknowledgement");
        let sender_session = agents.get(&sender_agent_id).and_then(|r| r.session.clone());

        Ok((pending.message_id, sender_session, sender_agent_id))
    }

    pub fn connected_sessions(&self, excluding_agent_id: &str) -> Vec<(String, S)> {
        let agents = self.agents.lock().expect("agent registry lock poisoned");
        agents
            .iter()
            .filter_map(|(agent_id, record)| {
                if agent_id == excluding_agent_id {
                    return None;
                }
                record.session.clone().map(|s| (agent_id.clone(), s))
            })
            .collect()
    }

    pub fn agent_session(&self, agent_id: &str) -> Option<S> {
        let agents = self.agents.lock().expect("agent registry lock poisoned");
        agents.get(agent_id).and_then(|r| r.session.clone())
    }
}
