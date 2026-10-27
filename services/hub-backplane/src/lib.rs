//! A NATS backplane that lets several hub instances act as one.
//!
//! Every hub pointed at the same NATS server shares three things:
//!
//! - an **agent directory** (`JetStream` KV bucket `et-agents`) recording, per agent, its connection state, its last
//!   known IP and the hub currently holding its socket;
//! - a **mailbox** (KV bucket `et-pending`) of unacknowledged direct messages per recipient, filed by the hub's
//!   [`MessageRetention`], so a message survives its recipient reconnecting to a different hub;
//! - a **delivery fabric** over core NATS subjects: `et.hub.<hub_id>` carries frames for one hub's agents and
//!   `et.broadcast` carries fan-out to every hub. The target agent and payload kind travel as headers and the frame
//!   bytes as the body, so binary relay needs no re-encoding.
//!
//! KV keys are the hex encoding of the agent id, because an id may contain dots, which NATS reads as token separators.
//! Every read-modify-write is a compare-and-set on the entry's revision, retried on conflict.

use std::collections::BTreeMap;
use std::future::ready;
use std::net::IpAddr;
use std::time::Duration;

use async_nats::jetstream::kv::{self, CreateErrorKind, Operation, Store, UpdateErrorKind};
use async_nats::{Client, HeaderMap, Message};
use bytes::Bytes;
use edge_toolkit::ws::{AgentConnectionState, AgentSummary};
use edge_toolkit::ws_server::{MessageRetention, PendingDirectMessage};
use futures_util::{Stream, StreamExt as _, TryStreamExt as _};
use serde::{Deserialize, Serialize};
use tracing::warn;
use uuid::Uuid;

mod config;
mod error;

pub use self::config::BackplaneConfig;
pub use self::error::BackplaneError;
use self::error::{MapConnectErr as _, MapNatsErr as _};

const AGENTS_BUCKET: &str = "et-agents";
const PENDING_BUCKET: &str = "et-pending";
const BROADCAST_SUBJECT: &str = "et.broadcast";
const TO_HEADER: &str = "Et-To";
const FROM_HEADER: &str = "Et-From";
const ORIGIN_HEADER: &str = "Et-Origin-Hub";
const KIND_HEADER: &str = "Et-Kind";

/// How long `subscribe` waits for the server to confirm its subscriptions; a healthy server takes one round trip.
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(10);

/// The backplane a hub routes through: nothing beyond this process, or a shared NATS server.
#[derive(Clone, Debug, Default)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the hub routes on each mode; a new one must make it say how"
)]
pub enum Backplane {
    /// Also what an app that never joined a backplane -- any embedding that predates them -- runs as.
    #[default]
    Local,
    Nats(Box<NatsBackplane>),
}

impl Backplane {
    /// Connect the backplane `config` names; `Local` needs nothing.
    pub async fn connect(config: &BackplaneConfig) -> Result<Self, BackplaneError> {
        match config {
            BackplaneConfig::Local => Ok(Self::Local),
            BackplaneConfig::Nats(url) => Ok(Self::Nats(Box::new(NatsBackplane::connect(url).await?))),
        }
    }

    /// Where the hubs on this backplane place `agent_id`.
    ///
    /// Always `Unknown` for `Local`, whose only registry is the caller's own.
    pub async fn presence(&self, agent_id: &str) -> Result<Presence, BackplaneError> {
        match self {
            Self::Local => Ok(Presence::Unknown),
            Self::Nats(nats) => nats.presence(agent_id).await,
        }
    }
}

/// Where the directory places an agent.
#[derive(Clone, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the hub routes on each placement; a new one must make it say how"
)]
pub enum Presence {
    /// No hub has ever registered it.
    Unknown,
    /// Its socket is open on the hub with id `hub_id`, from `last_known_ip`.
    Connected { hub_id: String, last_known_ip: IpAddr },
    /// It registered once and has since gone.
    Disconnected,
}

/// One websocket frame carried between hubs, written to the agent verbatim.
#[derive(Clone, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "mirrors the two websocket data frame types, which are fixed"
)]
pub enum Frame {
    Text(String),
    Binary(Bytes),
}

impl Frame {
    const fn kind(&self) -> &'static str {
        match self {
            Self::Text(_) => "text",
            Self::Binary(_) => "binary",
        }
    }

    fn into_payload(self) -> Bytes {
        match self {
            Self::Text(text) => Bytes::from(text),
            Self::Binary(bytes) => bytes,
        }
    }

    fn decode(message: &Message) -> Result<Self, BackplaneError> {
        match header(message, KIND_HEADER)? {
            "text" => Ok(Self::Text(String::from_utf8(message.payload.to_vec())?)),
            "binary" => Ok(Self::Binary(message.payload.clone())),
            other => Err(BackplaneError::UnknownKind(other.to_owned())),
        }
    }
}

/// A frame another hub sent for this hub's agents.
#[derive(Clone, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the hub delivers each kind; a new one must make it say how"
)]
pub enum Inbound {
    /// For the one local agent `to_agent_id`.
    Direct { to_agent_id: String, frame: Frame },
    /// For every local agent except `from_agent_id`.
    Broadcast { from_agent_id: String, frame: Frame },
}

impl Inbound {
    #[expect(
        clippy::single_call_fn,
        reason = "the decoder for one subscription; named beside its broadcast twin"
    )]
    fn decode_direct(message: &Message) -> Result<Self, BackplaneError> {
        Ok(Self::Direct {
            to_agent_id: header(message, TO_HEADER)?.to_owned(),
            frame: Frame::decode(message)?,
        })
    }

    #[expect(
        clippy::single_call_fn,
        reason = "the decoder for one subscription; named beside its direct twin"
    )]
    fn decode_broadcast(message: &Message) -> Result<Self, BackplaneError> {
        Ok(Self::Broadcast {
            from_agent_id: header(message, FROM_HEADER)?.to_owned(),
            frame: Frame::decode(message)?,
        })
    }
}

fn header<'message>(message: &'message Message, name: &'static str) -> Result<&'message str, BackplaneError> {
    message
        .headers
        .as_ref()
        .and_then(|headers| headers.get(name))
        .map(async_nats::HeaderValue::as_str)
        .ok_or(BackplaneError::MissingHeader(name))
}

/// One agent's directory entry.
#[derive(Debug, Deserialize, Serialize)]
struct DirectoryRecord {
    agent_id: String,
    state: AgentConnectionState,
    last_known_ip: IpAddr,
    hub_id: String,
}

/// One recipient's unacknowledged messages, keyed by [`MessageRetention::key`].
type Mailbox = BTreeMap<String, PendingDirectMessage>;

/// A hub's connection to a shared NATS backplane.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct NatsBackplane {
    client: Client,
    agents: Store,
    pending: Store,
    hub_id: String,
}

impl NatsBackplane {
    /// Connect to the NATS server at `url`, creating the two KV buckets if this is the first hub to arrive.
    pub async fn connect(url: &str) -> Result<Self, BackplaneError> {
        let client = async_nats::connect(url).await.map_connect_err(url)?;
        let jetstream = async_nats::jetstream::new(client.clone());
        let agents = jetstream
            .create_or_update_key_value(kv::Config {
                bucket: AGENTS_BUCKET.to_owned(),
                ..kv::Config::default()
            })
            .await
            .map_nats_err("open the agent directory")?;
        let pending = jetstream
            .create_or_update_key_value(kv::Config {
                bucket: PENDING_BUCKET.to_owned(),
                ..kv::Config::default()
            })
            .await
            .map_nats_err("open the mailbox")?;
        Ok(Self {
            client,
            agents,
            pending,
            hub_id: Uuid::now_v7().to_string(),
        })
    }

    /// This hub's id, unique per process.
    #[must_use]
    pub fn hub_id(&self) -> &str {
        &self.hub_id
    }

    /// The largest message, in bytes, the broker accepts: no frame bigger than this can reach another hub.
    #[must_use]
    pub fn max_payload(&self) -> usize {
        self.client.server_info().max_payload
    }

    /// Record `agent_id` as connected to this hub, taking it over from whichever hub held it before.
    pub async fn announce(&self, agent_id: &str, last_known_ip: IpAddr) -> Result<(), BackplaneError> {
        let record = DirectoryRecord {
            agent_id: agent_id.to_owned(),
            state: AgentConnectionState::Connected,
            last_known_ip,
            hub_id: self.hub_id.clone(),
        };
        let _revision = self
            .agents
            .put(key(agent_id), serde_json::to_vec(&record)?.into())
            .await
            .map_nats_err("announce an agent")?;
        Ok(())
    }

    /// Record `agent_id` as disconnected, but only while the directory still places it on this hub.
    ///
    /// An agent that has since reconnected to another hub belongs to that hub now; this hub's socket closing late
    /// must not mark its replacement gone. The check and the write are one compare-and-set, so a takeover landing in
    /// between makes the write fail and the retry then sees the new owner.
    pub async fn withdraw(&self, agent_id: &str) -> Result<(), BackplaneError> {
        loop {
            let Some((mut record, revision)) = self.directory_entry(agent_id).await? else {
                return Ok(());
            };
            if record.hub_id != self.hub_id || record.state == AgentConnectionState::Disconnected {
                return Ok(());
            }
            record.state = AgentConnectionState::Disconnected;
            match self
                .agents
                .update(key(agent_id), serde_json::to_vec(&record)?.into(), revision)
                .await
            {
                Ok(_revision) => return Ok(()),
                Err(error) if matches!(error.kind(), UpdateErrorKind::WrongLastRevision) => {}
                Err(error) => return Err(error).map_nats_err("withdraw an agent"),
            }
        }
    }

    /// Where the directory places `agent_id`.
    pub async fn presence(&self, agent_id: &str) -> Result<Presence, BackplaneError> {
        Ok(match self.directory_entry(agent_id).await? {
            None => Presence::Unknown,
            Some((record, _revision)) if record.state == AgentConnectionState::Connected => Presence::Connected {
                hub_id: record.hub_id,
                last_known_ip: record.last_known_ip,
            },
            Some(_disconnected) => Presence::Disconnected,
        })
    }

    /// Every agent any hub on this backplane has registered, sorted by id.
    pub async fn list_agents(&self) -> Result<Vec<AgentSummary>, BackplaneError> {
        let keys: Vec<String> = self
            .agents
            .keys()
            .await
            .map_nats_err("list the agent directory")?
            .try_collect()
            .await
            .map_nats_err("read the agent directory")?;
        let mut agents = Vec::with_capacity(keys.len());
        for encoded in keys {
            let agent_id = String::from_utf8(const_hex::decode(&encoded)?)?;
            if let Some((record, _revision)) = self.directory_entry(&agent_id).await? {
                agents.push(AgentSummary::new(record.agent_id, record.state, record.last_known_ip));
            }
        }
        agents.sort_by(|left, right| left.agent_id.cmp(&right.agent_id));
        Ok(agents)
    }

    /// Put `message` in `to_agent_id`'s mailbox, replacing an earlier one from the same sender if `retention` says to.
    pub async fn queue(
        &self,
        to_agent_id: &str,
        message: PendingDirectMessage,
        retention: MessageRetention,
    ) -> Result<(), BackplaneError> {
        self.modify_mailbox(to_agent_id, |mailbox| {
            let _previous = mailbox.insert(retention.key(&message), message.clone());
            Ok(())
        })
        .await
    }

    /// Everything waiting in `agent_id`'s mailbox, oldest message id first.
    pub async fn pending_for(&self, agent_id: &str) -> Result<Vec<PendingDirectMessage>, BackplaneError> {
        let (mailbox, _revision) = self.read_mailbox(&key(agent_id)).await?;
        let mut pending = mailbox.into_values().collect::<Vec<_>>();
        pending.sort_by(|left, right| left.message_id.cmp(&right.message_id));
        Ok(pending)
    }

    /// Take message `message_id` out of `recipient_agent_id`'s mailbox, returning the id of the agent that sent it.
    pub async fn acknowledge(&self, recipient_agent_id: &str, message_id: &str) -> Result<String, BackplaneError> {
        self.modify_mailbox(recipient_agent_id, |mailbox| {
            let (key, sender) = mailbox
                .iter()
                .find_map(|(key, pending)| {
                    (pending.message_id == message_id).then(|| (key.clone(), pending.from_agent_id.clone()))
                })
                .ok_or_else(|| BackplaneError::NoPendingMessage(message_id.to_owned()))?;
            let _acknowledged = mailbox.remove(&key);
            Ok(sender)
        })
        .await
    }

    /// Send `frame` to `to_agent_id` through the hub with id `hub_id`.
    pub async fn send_to_hub(&self, hub_id: &str, to_agent_id: &str, frame: Frame) -> Result<(), BackplaneError> {
        let mut headers = HeaderMap::new();
        headers.insert(TO_HEADER, to_agent_id);
        headers.insert(KIND_HEADER, frame.kind());
        self.client
            .publish_with_headers(hub_subject(hub_id), headers, frame.into_payload())
            .await
            .map_nats_err("send to another hub")
    }

    /// Send `frame` from `from_agent_id` to the agents of every other hub.
    pub async fn broadcast(&self, from_agent_id: &str, frame: Frame) -> Result<(), BackplaneError> {
        let mut headers = HeaderMap::new();
        headers.insert(FROM_HEADER, from_agent_id);
        headers.insert(ORIGIN_HEADER, self.hub_id.as_str());
        headers.insert(KIND_HEADER, frame.kind());
        self.client
            .publish_with_headers(BROADCAST_SUBJECT, headers, frame.into_payload())
            .await
            .map_nats_err("broadcast to the other hubs")
    }

    /// Wait until the server has registered every subscription this connection has sent.
    ///
    /// The server registers a subscription only once it reads the SUB, and a frame another hub publishes before then
    /// is dropped, since core NATS keeps nothing for a subject with no subscriber. A flush does not wait for that: it
    /// resolves once the bytes leave this process. So a message is sent to a fresh inbox this connection subscribes
    /// to, and awaited: the server handles a connection's commands in order, so the echo arriving means every SUB
    /// before it has been read.
    ///
    /// The wait is bounded, because an echo can be lost rather than late: a reconnect between the publish and its
    /// delivery drops it, and an account not allowed to publish to its inboxes never sends it. Either way the caller
    /// gets an error to act on, rather than a hub that never finishes starting.
    async fn confirm_subscriptions(&self) -> Result<(), BackplaneError> {
        let inbox = self.client.new_inbox();
        let mut echo = self
            .client
            .subscribe(inbox.clone())
            .await
            .map_nats_err("subscribe to the confirmation inbox")?;
        self.client
            .publish(inbox, Bytes::new())
            .await
            .map_nats_err("publish to the confirmation inbox")?;
        let echoed = tokio::time::timeout(CONFIRM_TIMEOUT, echo.next()).await;
        let _echoed: Message = echoed
            .ok()
            .flatten()
            .ok_or(BackplaneError::SubscriptionsUnconfirmed(CONFIRM_TIMEOUT))?;
        echo.unsubscribe()
            .await
            .map_nats_err("unsubscribe from the confirmation inbox")
    }

    /// The frames other hubs send this hub's agents, from the moment this returns.
    ///
    /// This hub's own broadcasts are left out, since it has already fanned them out locally. A message that does not
    /// decode is logged and skipped rather than ending the stream, so one bad publisher cannot cut a hub off.
    pub async fn subscribe(&self) -> Result<impl Stream<Item = Inbound> + Send + 'static, BackplaneError> {
        let direct = self
            .client
            .subscribe(hub_subject(&self.hub_id))
            .await
            .map_nats_err("subscribe to this hub's subject")?;
        let broadcast = self
            .client
            .subscribe(BROADCAST_SUBJECT)
            .await
            .map_nats_err("subscribe to broadcasts")?;
        self.confirm_subscriptions().await?;

        let own_hub = self.hub_id.clone();
        let direct = direct.map(|message| Inbound::decode_direct(&message));
        let broadcast = broadcast
            .filter(move |message| ready(header(message, ORIGIN_HEADER).ok() != Some(own_hub.as_str())))
            .map(|message| Inbound::decode_broadcast(&message));
        Ok(futures_util::stream::select(direct, broadcast).filter_map(|decoded| {
            ready(
                decoded
                    .inspect_err(|error| warn!("Dropping undecodable backplane message: {error}"))
                    .ok(),
            )
        }))
    }

    /// The directory entry for `agent_id` with its revision, or nothing if no hub has registered it.
    async fn directory_entry(&self, agent_id: &str) -> Result<Option<(DirectoryRecord, u64)>, BackplaneError> {
        match self
            .agents
            .entry(key(agent_id))
            .await
            .map_nats_err("read the agent directory")?
        {
            Some(entry) if entry.operation == Operation::Put => {
                Ok(Some((serde_json::from_slice(&entry.value)?, entry.revision)))
            }
            Some(_) | None => Ok(None),
        }
    }

    /// Apply `change` to `agent_id`'s mailbox as one compare-and-set, re-reading and re-applying on conflict.
    async fn modify_mailbox<R>(
        &self,
        agent_id: &str,
        mut change: impl FnMut(&mut Mailbox) -> Result<R, BackplaneError>,
    ) -> Result<R, BackplaneError> {
        let key = key(agent_id);
        loop {
            let (mut mailbox, revision) = self.read_mailbox(&key).await?;
            let result = change(&mut mailbox)?;
            if self.write_mailbox(&key, &mailbox, revision).await? {
                return Ok(result);
            }
        }
    }

    /// The mailbox stored under `key` and the revision it was read at; empty and [`MailboxRevision::New`] if none.
    async fn read_mailbox(&self, key: &str) -> Result<(Mailbox, MailboxRevision), BackplaneError> {
        match self.pending.entry(key).await.map_nats_err("read a mailbox")? {
            Some(entry) if entry.operation == Operation::Put => Ok((
                serde_json::from_slice(&entry.value)?,
                MailboxRevision::Stored(entry.revision),
            )),
            Some(_) | None => Ok((Mailbox::new(), MailboxRevision::New)),
        }
    }

    /// Store `mailbox` under `key` if it is still at `revision`, returning false when another writer got there first.
    async fn write_mailbox(
        &self,
        key: &str,
        mailbox: &Mailbox,
        revision: MailboxRevision,
    ) -> Result<bool, BackplaneError> {
        let value = Bytes::from(serde_json::to_vec(mailbox)?);
        match revision {
            MailboxRevision::Stored(revision) => match self.pending.update(key, value, revision).await {
                Ok(_revision) => Ok(true),
                Err(error) if matches!(error.kind(), UpdateErrorKind::WrongLastRevision) => Ok(false),
                Err(error) => Err(error).map_nats_err("write a mailbox"),
            },
            MailboxRevision::New => match self.pending.create(key, value).await {
                Ok(_revision) => Ok(true),
                Err(error) if matches!(error.kind(), CreateErrorKind::AlreadyExists) => Ok(false),
                Err(error) => Err(error).map_nats_err("create a mailbox"),
            },
        }
    }
}

/// Which revision of a mailbox a read saw, so the write that follows can be a compare-and-set against it.
#[derive(Clone, Copy, Debug)]
enum MailboxRevision {
    /// No mailbox was stored, so the write must be the one that creates it.
    New,
    /// The mailbox was stored, at this revision.
    Stored(u64),
}

fn key(agent_id: &str) -> String {
    const_hex::encode(agent_id)
}

fn hub_subject(hub_id: &str) -> String {
    format!("et.hub.{hub_id}")
}
