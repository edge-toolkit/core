use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{info, warn};

use crate::ws::{AgentConnectionState, AgentSummary, ConnectStatus};

/// Longest agent id an agent may choose for itself; a generated UUID is 36 characters.
pub const MAX_AGENT_ID_LEN: usize = 64;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RegistryError {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("failed to (de)serialize registry YAML")]
    Yaml(#[from] serde_yaml::Error),

    #[error("agent registry lock poisoned")]
    LockPoisoned,

    #[error("unknown agent {0}")]
    UnknownAgent(String),
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

impl PendingDirectMessage {
    /// A direct message `from_agent_id` sent, stamped with the hub's id and receipt time.
    #[must_use]
    pub const fn new(
        message_id: String,
        from_agent_id: String,
        server_received_at: String,
        message: serde_json::Value,
    ) -> Self {
        Self {
            message_id,
            from_agent_id,
            server_received_at,
            message,
        }
    }
}

/// How many unacknowledged direct messages a recipient holds from each sender.
///
/// The default keeps one: a recipient's pending messages then number at most its senders, so the memory they take is
/// bounded and known in advance. Agents that must not lose a message wait for the acknowledgement of one before sending
/// the next; that discipline is theirs, not the hub's.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum MessageRetention {
    /// Only the newest message from each sender; a second one replaces the first, which is never delivered.
    #[default]
    LatestPerSender,
    /// Every message until it is acknowledged. Nothing bounds how many a recipient holds.
    All,
}

impl MessageRetention {
    /// The key `pending` is filed under in its recipient's pending messages.
    #[must_use]
    pub fn key(self, pending: &PendingDirectMessage) -> String {
        match self {
            Self::LatestPerSender => pending.from_agent_id.clone(),
            Self::All => pending.message_id.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct AgentRecord<S> {
    pub state: AgentConnectionState,
    pub last_known_ip: IpAddr,
    #[serde(skip)]
    pub session: Option<S>,
    /// Unacknowledged direct messages, keyed by [`MessageRetention::key`].
    ///
    /// Entries are found by the message id they carry rather than by key, so a registry saved under one retention mode
    /// still acknowledges after a restart under the other.
    #[serde(default)]
    pub pending_direct_messages: BTreeMap<String, PendingDirectMessage>,
}

impl<S> AgentRecord<S> {
    /// Construct a record with no pending direct messages.
    #[must_use]
    pub const fn new(state: AgentConnectionState, last_known_ip: IpAddr, session: Option<S>) -> Self {
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

/// The client addresses a hub refuses, shared by every clone that holds it.
///
/// Every address is held and compared in its canonical form, so an IPv4-mapped IPv6 address (`::ffff:192.0.2.7`)
/// and the IPv4 address it maps (`192.0.2.7`) are one entry, whichever spelling blocked it or connects with it.
///
/// Not generic over the session type, unlike the registry carrying it, so each method here is one copy that the
/// branch-coverage gate scores once rather than once per session type.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct IpBlocklist(Arc<Mutex<BTreeSet<IpAddr>>>);

impl IpBlocklist {
    /// A blocklist refusing exactly `ips`.
    #[must_use]
    pub fn from_ips(ips: BTreeSet<IpAddr>) -> Self {
        Self(Arc::new(Mutex::new(
            ips.into_iter().map(|ip| ip.to_canonical()).collect(),
        )))
    }

    /// Refuse `ip` from now on, returning whether it was not refused already.
    pub fn block(&self, ip: IpAddr) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(ip.to_canonical())
    }

    /// Whether `ip` is refused.
    #[must_use]
    pub fn is_blocked(&self, ip: IpAddr) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&ip.to_canonical())
    }

    /// Every refused address, in order.
    #[must_use]
    pub fn ips(&self) -> BTreeSet<IpAddr> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// A registry as it is laid out on disk: its agents keyed by id, and the client addresses it refuses.
///
/// `A` is the agent map, owned when read and borrowed when written.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct RegistryFile<A> {
    pub agents: A,
    #[serde(default)]
    pub blocked_ips: BTreeSet<IpAddr>,
}

/// Either registry layout: the current one, or the bare agent map written before the hub kept a blocklist.
///
/// `deny_unknown_fields` on [`RegistryFile`] is what tells them apart, since a bare map is keyed by agent ids.
#[derive(Deserialize)]
#[serde(untagged)]
enum StoredRegistry<R> {
    Current(RegistryFile<BTreeMap<String, R>>),
    Bare(BTreeMap<String, R>),
}

/// Read the registry file at `path`, whose agents are records of type `R`; a missing file is an empty registry.
pub fn read_registry_file<R>(path: &std::path::Path) -> Result<RegistryFile<BTreeMap<String, R>>, RegistryError>
where
    R: DeserializeOwned,
{
    if !path.exists() {
        warn!(
            "Registry file {} does not exist, starting with empty registry",
            path.display()
        );
        return Ok(RegistryFile {
            agents: BTreeMap::new(),
            blocked_ips: BTreeSet::new(),
        });
    }
    let file = parse_registry(&fs_err::read_to_string(path)?)?;
    info!(
        "Loaded {} agents and {} blocked IPs from registry {}",
        file.agents.len(),
        file.blocked_ips.len(),
        path.display()
    );
    Ok(file)
}

/// Parse a registry file's YAML in either layout, giving a bare agent map an empty blocklist.
#[expect(
    clippy::single_call_fn,
    reason = "the parsing step of read_registry_file, kept apart from its file handling and logging"
)]
fn parse_registry<R>(yaml: &str) -> Result<RegistryFile<BTreeMap<String, R>>, RegistryError>
where
    R: DeserializeOwned,
{
    Ok(match serde_yaml::from_str(yaml)? {
        StoredRegistry::Current(file) => file,
        StoredRegistry::Bare(agents) => RegistryFile {
            agents,
            blocked_ips: BTreeSet::new(),
        },
    })
}

#[derive(Clone)]
#[non_exhaustive]
pub struct AgentRegistry<S> {
    pub agents: Arc<Mutex<BTreeMap<String, AgentRecord<S>>>>,
    pub blocklist: IpBlocklist,
}

impl<S> AgentRegistry<S> {
    /// Wrap a populated agent map.
    #[must_use]
    pub fn from_agents(agents: BTreeMap<String, AgentRecord<S>>) -> Self {
        Self {
            agents: Arc::new(Mutex::new(agents)),
            blocklist: IpBlocklist::default(),
        }
    }

    /// Replace the blocklist (chainable; used by persistence reloaders).
    #[must_use]
    pub fn with_blocklist(mut self, blocklist: IpBlocklist) -> Self {
        self.blocklist = blocklist;
        self
    }
}

impl<S: Clone + Default + Send + 'static> AgentRegistry<S> {
    pub fn load(path: &std::path::Path) -> Result<Self, RegistryError> {
        let file = read_registry_file::<AgentRecord<S>>(path)?;
        Ok(Self::from_agents(file.agents).with_blocklist(IpBlocklist::from_ips(file.blocked_ips)))
    }
}

impl<S: Clone + Send + 'static> AgentRegistry<S> {
    pub fn save(&self, path: &std::path::Path) -> Result<(), RegistryError> {
        let agents = self.agents.lock()?;
        let yaml = serde_yaml::to_string(&RegistryFile {
            agents: &*agents,
            blocked_ips: self.blocklist.ips(),
        })?;
        drop(agents);
        fs_err::write(path, yaml)?;
        info!("Agent registry saved to {}", path.display());
        Ok(())
    }

    /// The address `agent_id` last connected from.
    pub fn agent_ip(&self, agent_id: &str) -> Result<IpAddr, RegistryError> {
        lock_agents(&self.agents)
            .get(agent_id)
            .map(|record| record.last_known_ip)
            .ok_or_else(|| RegistryError::UnknownAgent(agent_id.to_string()))
    }

    pub fn connect_agent(
        &self,
        requested_id: Option<String>,
        new_id: String,
        client_ip: IpAddr,
        session: S,
    ) -> (String, ConnectStatus) {
        let mut agents = lock_agents(&self.agents);

        if let Some(requested) = requested_id.as_deref()
            && let Some(record) = agents.get_mut(requested)
        {
            record.state = AgentConnectionState::Connected;
            record.last_known_ip = client_ip;
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
                last_known_ip: client_ip,
                session: Some(session),
                pending_direct_messages: BTreeMap::new(),
            },
        );
        drop(agents);
        (assigned_id, ConnectStatus::Assigned)
    }

    pub fn mark_disconnected(&self, agent_id: &str) {
        let _marked = self.mark_disconnected_if(agent_id, &|_session| true);
    }

    /// Mark `agent_id` disconnected only if `is_current` accepts the session the registry holds for it.
    ///
    /// A connection that takes over an id another connection still holds -- the newest claim wins, so a reconnect whose
    /// old socket has not closed yet keeps its identity -- replaces that session. When the displaced connection later
    /// closes, it must not disconnect its replacement, so a closing connection passes a predicate for its own session.
    /// A record whose session is already gone is disconnected whatever the predicate says, since nothing live holds it.
    ///
    /// The predicate is a trait object rather than a generic so there is one copy of this per session type.
    /// Every closure is its own type, and a generic would give each caller its own copy with only that caller's
    /// branches taken -- which the branch-coverage gate scores copy by copy, so no single copy would ever read
    /// as fully covered: `libs/edge-toolkit/src/ws_server.rs 19/20 branches` on commit
    /// <https://github.com/edge-toolkit/core/commit/1782e4517b8c16d714d5c5e6e83d0078f2a869a9> at
    /// <https://github.com/edge-toolkit/core/actions/runs/36699637919/job/109835793913>.
    ///
    /// Returns whether the record was marked, so a caller mirroring the registry elsewhere skips the same writes.
    pub fn mark_disconnected_if(&self, agent_id: &str, is_current: &dyn Fn(&S) -> bool) -> bool {
        let mut agents = lock_agents(&self.agents);
        let marked = agents
            .get_mut(agent_id)
            .filter(|record| record.session.as_ref().is_none_or(is_current))
            .map(|record| {
                record.state = AgentConnectionState::Disconnected;
                record.session = None;
            })
            .is_some();
        drop(agents);
        marked
    }

    #[must_use]
    pub fn list_agents(&self) -> Vec<AgentSummary> {
        let agents = lock_agents(&self.agents);
        let mut summaries = agents
            .iter()
            .map(|(agent_id, record)| AgentSummary {
                agent_id: agent_id.clone(),
                state: record.state.clone(),
                last_known_ip: record.last_known_ip,
            })
            .collect::<Vec<_>>();
        drop(agents);
        summaries.sort_by(|left, right| left.agent_id.cmp(&right.agent_id));
        summaries
    }

    /// Queue a direct message for `to_agent_id`, returning the stored message and the recipient's session.
    ///
    /// `retention` decides whether an earlier unacknowledged message from the same sender is replaced or kept.
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
        retention: MessageRetention,
    ) -> Option<(PendingDirectMessage, Option<S>)> {
        let mut agents = lock_agents(&self.agents);
        let recipient = agents.get_mut(to_agent_id)?;

        let pending = PendingDirectMessage::new(message_id, from_agent_id.to_string(), server_received_at, message);
        let session = recipient.session.clone();
        let _previous: Option<PendingDirectMessage> = recipient
            .pending_direct_messages
            .insert(retention.key(&pending), pending.clone());
        drop(agents);

        Some((pending, session))
    }

    /// Drop every queued message `from_agent_id` sent, from every recipient's queue, returning how many were dropped.
    #[must_use]
    pub fn forget_messages_from(&self, from_agent_id: &str) -> usize {
        let mut agents = lock_agents(&self.agents);
        let dropped = agents
            .values_mut()
            .map(|record| {
                let before = record.pending_direct_messages.len();
                record
                    .pending_direct_messages
                    .retain(|_key, pending| pending.from_agent_id != from_agent_id);
                before.saturating_sub(record.pending_direct_messages.len())
            })
            .sum();
        drop(agents);
        dropped
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

        let key = recipient
            .pending_direct_messages
            .iter()
            .find_map(|(key, pending)| (pending.message_id == message_id).then(|| key.clone()))
            .ok_or(AcknowledgeError::NoPendingMessage)?;
        // The `find_map` above drops its iterator before we re-borrow `pending_direct_messages` mutably for the
        // removal. The double lookup costs O(log n) but lets us share the `NoPendingMessage` error with the find-side
        // case instead of asserting an invariant.
        let pending = recipient
            .pending_direct_messages
            .remove(&key)
            .ok_or(AcknowledgeError::NoPendingMessage)?;
        let sender_agent_id = pending.from_agent_id;
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
            blocklist: IpBlocklist::default(),
        }
    }
}
