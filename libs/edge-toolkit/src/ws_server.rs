use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{info, warn};

use crate::ws::{AgentConnectionState, AgentSummary, ConnectStatus};

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RegistryError {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("failed to (de)serialize registry YAML")]
    Yaml(#[from] serde_yaml::Error),

    #[error("agent registry lock poisoned")]
    LockPoisoned,
}

impl<T> From<PoisonError<T>> for RegistryError {
    fn from(_source: PoisonError<T>) -> Self {
        Self::LockPoisoned
    }
}

/// Why an `acknowledge_message` call rejected the ack.
///
/// The variant itself describes *what* went wrong; the optional payload is the recipient/sender id the caller can quote
/// back in a wire-level status message.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AcknowledgeError {
    #[error("unknown acknowledging agent {0}")]
    UnknownAgent(String),

    #[error("no pending message to acknowledge")]
    NoPendingMessage,

    #[error("agent registry lock poisoned")]
    LockPoisoned,
}

impl<T> From<PoisonError<T>> for AcknowledgeError {
    fn from(_source: PoisonError<T>) -> Self {
        Self::LockPoisoned
    }
}

/// Take the lock, recovering from poison by returning the inner guard.
///
/// We never observe poisoned state in the wild -- every panic-prone path holds the lock briefly around infallible map
/// ops. Recovering keeps the registry usable if a future change introduces a panic under the lock.
fn lock_agents<S>(
    agents: &Mutex<BTreeMap<String, AgentRecord<S>>>,
) -> MutexGuard<'_, BTreeMap<String, AgentRecord<S>>> {
    agents.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Longest agent id an agent may choose for itself; a generated UUID is 36 characters.
pub const MAX_AGENT_ID_LEN: usize = 64;

/// Whether `id` is acceptable as an agent id the agent chose itself rather than one the hub generated.
///
/// The id becomes the first segment of the agent's storage paths (`<agent_id>/<file>`), so it is held to characters
/// that cannot address anything outside that bucket: ASCII letters, digits, `.`, `_` and `-`, with no `/`, and not a
/// dot-only segment such as `..`. Every id the hub generates passes, so a reconnecting agent is never affected.
#[must_use]
pub fn is_valid_agent_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_AGENT_ID_LEN
        && id.chars().any(|character| character != '.')
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-'))
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct PendingDirectMessage {
    pub message_id: String,
    pub from_agent_id: String,
    pub server_received_at: String,
    pub message: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct AgentRecord<S> {
    pub state: AgentConnectionState,
    pub last_known_ip: Option<String>,
    #[serde(skip)]
    pub session: Option<S>,
    #[serde(default)]
    pub pending_direct_messages: BTreeMap<String, PendingDirectMessage>,
}

impl<S> AgentRecord<S> {
    /// Construct a record with no pending direct messages.
    #[must_use]
    pub const fn new(state: AgentConnectionState, last_known_ip: Option<String>, session: Option<S>) -> Self {
        Self {
            state,
            last_known_ip,
            session,
            pending_direct_messages: BTreeMap::new(),
        }
    }

    /// Replace `pending_direct_messages` (chainable; used by persistence reloaders).
    #[must_use]
    pub fn with_pending_direct_messages(mut self, pending: BTreeMap<String, PendingDirectMessage>) -> Self {
        self.pending_direct_messages = pending;
        self
    }
}

#[derive(Clone)]
#[non_exhaustive]
pub struct AgentRegistry<S> {
    pub agents: Arc<Mutex<BTreeMap<String, AgentRecord<S>>>>,
}

impl<S> AgentRegistry<S> {
    /// Wrap a populated agent map; useful for tests building fixtures.
    #[must_use]
    pub fn from_agents(agents: BTreeMap<String, AgentRecord<S>>) -> Self {
        Self {
            agents: Arc::new(Mutex::new(agents)),
        }
    }
}

impl<S: Clone + Default + Send + 'static> AgentRegistry<S> {
    pub fn load(path: &std::path::Path) -> Result<Self, RegistryError> {
        if !path.exists() {
            warn!(
                "Registry file {} does not exist, starting with empty registry",
                path.display()
            );
            return Ok(Self::default());
        }
        let yaml = fs_err::read_to_string(path)?;
        let agents: BTreeMap<String, AgentRecord<S>> = serde_yaml::from_str(&yaml)?;
        info!("Loaded {} agents from registry {}", agents.len(), path.display());
        Ok(Self {
            agents: Arc::new(Mutex::new(agents)),
        })
    }
}

impl<S: Clone + Send + 'static> AgentRegistry<S> {
    pub fn save(&self, path: &std::path::Path) -> Result<(), RegistryError> {
        let agents = self.agents.lock()?;
        let yaml = serde_yaml::to_string(&*agents)?;
        drop(agents);
        fs_err::write(path, yaml)?;
        info!("Agent registry saved to {}", path.display());
        Ok(())
    }

    pub fn connect_agent(
        &self,
        requested_id: Option<String>,
        new_id: String,
        client_ip: &str,
        session: S,
    ) -> (String, ConnectStatus) {
        let mut agents = lock_agents(&self.agents);

        if let Some(requested) = requested_id.as_deref()
            && let Some(record) = agents.get_mut(requested)
        {
            record.state = AgentConnectionState::Connected;
            record.last_known_ip = Some(client_ip.to_string());
            record.session = Some(session);
            return (requested.to_string(), ConnectStatus::Reconnected);
        }

        // An id the hub has never seen is taken as the agent's own name for itself, so a headless agent can keep one
        // identity -- and one storage bucket -- across hub restarts without the hub persisting its registry. One that
        // is not safe to use as a storage path segment gets a generated id instead, as before.
        let assigned_id = requested_id.filter(|id| is_valid_agent_id(id)).unwrap_or(new_id);
        let _previous: Option<AgentRecord<S>> = agents.insert(
            assigned_id.clone(),
            AgentRecord {
                state: AgentConnectionState::Connected,
                last_known_ip: Some(client_ip.to_string()),
                session: Some(session),
                pending_direct_messages: BTreeMap::new(),
            },
        );
        drop(agents);
        (assigned_id, ConnectStatus::Assigned)
    }

    pub fn mark_disconnected(&self, agent_id: &str) {
        self.mark_disconnected_if(agent_id, &|_session| true);
    }

    /// Mark `agent_id` disconnected only if `is_current` accepts the session the registry holds for it.
    ///
    /// A connection that takes over an id another connection still holds -- the newest claim wins, so a reconnect whose
    /// old socket has not closed yet keeps its identity -- replaces that session. When the displaced connection later
    /// closes, it must not disconnect its replacement, so a closing connection passes a test for its own session. A
    /// record whose session is already gone is disconnected whatever the test says, since nothing live holds it.
    ///
    /// The test is a trait object rather than a generic so there is one copy of this per session type. Every closure is
    /// its own type, and a generic would give each caller its own copy with only that caller's branches taken -- which
    /// the branch-coverage gate scores copy by copy, so no single copy would ever read as fully covered:
    /// `libs/edge-toolkit/src/ws_server.rs 19/20 branches` on commit
    /// <https://github.com/edge-toolkit/core/commit/1782e4517b8c16d714d5c5e6e83d0078f2a869a9> at
    /// <https://github.com/edge-toolkit/core/actions/runs/36699637919/job/109835793913>.
    pub fn mark_disconnected_if(&self, agent_id: &str, is_current: &dyn Fn(&S) -> bool) {
        let mut agents = lock_agents(&self.agents);
        if let Some(record) = agents.get_mut(agent_id)
            && record.session.as_ref().is_none_or(is_current)
        {
            record.state = AgentConnectionState::Disconnected;
            record.session = None;
        }
    }

    #[must_use]
    pub fn list_agents(&self) -> Vec<AgentSummary> {
        let agents = lock_agents(&self.agents);
        let mut summaries = agents
            .iter()
            .map(|(agent_id, record)| AgentSummary {
                agent_id: agent_id.clone(),
                state: record.state.clone(),
                last_known_ip: record.last_known_ip.clone(),
            })
            .collect::<Vec<_>>();
        drop(agents);
        summaries.sort_by(|left, right| left.agent_id.cmp(&right.agent_id));
        summaries
    }

    /// Queue a direct message for `to_agent_id`, returning the stored message and the recipient's session.
    ///
    /// Returns `None` when `to_agent_id` is not in the registry. The inner `Option<S>` is the recipient's live session
    /// -- `Some` when connected, `None` when the message was queued for a disconnected agent.
    #[must_use]
    pub fn queue_direct(
        &self,
        message_id: String,
        from_agent_id: &str,
        to_agent_id: &str,
        server_received_at: String,
        message: serde_json::Value,
    ) -> Option<(PendingDirectMessage, Option<S>)> {
        let mut agents = lock_agents(&self.agents);
        let recipient = agents.get_mut(to_agent_id)?;

        let pending = PendingDirectMessage {
            message_id,
            from_agent_id: from_agent_id.to_string(),
            server_received_at,
            message,
        };
        let session = recipient.session.clone();
        let _previous: Option<PendingDirectMessage> = recipient
            .pending_direct_messages
            .insert(from_agent_id.to_string(), pending.clone());
        drop(agents);

        Some((pending, session))
    }

    #[must_use]
    pub fn pending_messages_for(&self, agent_id: &str) -> Vec<PendingDirectMessage> {
        let agents = lock_agents(&self.agents);
        agents
            .get(agent_id)
            .map(|record| {
                let mut pending = record.pending_direct_messages.values().cloned().collect::<Vec<_>>();
                pending.sort_by(|left, right| left.message_id.cmp(&right.message_id));
                pending
            })
            .unwrap_or_default()
    }

    /// Returns `(message_id, sender_session, sender_agent_id)` on success.
    pub fn acknowledge_message(
        &self,
        recipient_agent_id: &str,
        message_id: &str,
    ) -> Result<(String, Option<S>, String), AcknowledgeError> {
        let mut agents = self.agents.lock()?;
        let recipient = agents
            .get_mut(recipient_agent_id)
            .ok_or_else(|| AcknowledgeError::UnknownAgent(recipient_agent_id.to_string()))?;

        let sender_agent_id = recipient
            .pending_direct_messages
            .iter()
            .find_map(|(id, pending)| (pending.message_id == message_id).then(|| id.clone()))
            .ok_or(AcknowledgeError::NoPendingMessage)?;
        // The `find_map` above drops its iterator before we re-borrow `pending_direct_messages` mutably for the
        // removal. The double lookup costs O(log n) but lets us share the `NoPendingMessage` error with the find-side
        // case instead of asserting an invariant.
        let pending = recipient
            .pending_direct_messages
            .remove(&sender_agent_id)
            .ok_or(AcknowledgeError::NoPendingMessage)?;
        let sender_session = agents.get(&sender_agent_id).and_then(|record| record.session.clone());
        drop(agents);

        Ok((pending.message_id, sender_session, sender_agent_id))
    }

    #[must_use]
    pub fn connected_sessions(&self, excluding_agent_id: &str) -> Vec<(String, S)> {
        let agents = lock_agents(&self.agents);
        agents
            .iter()
            .filter_map(|(agent_id, record)| {
                if agent_id == excluding_agent_id {
                    return None;
                }
                record.session.clone().map(|session| (agent_id.clone(), session))
            })
            .collect()
    }

    #[must_use]
    pub fn agent_session(&self, agent_id: &str) -> Option<S> {
        let agents = lock_agents(&self.agents);
        let session = agents.get(agent_id).and_then(|record| record.session.clone());
        drop(agents);
        session
    }
}

impl<S> Default for AgentRegistry<S> {
    fn default() -> Self {
        Self {
            agents: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }
}
