//! The hub's routing of agents' frames, apart from any transport, clock or id source.
//!
//! An embedder owns the sockets. For each connection it keeps a [`Link`], hands it every frame the agent sends, and
//! carries out the [`Routing`] that comes back: [`Routing::deliveries`] go to the sessions they name, the
//! [`Routing::shared`] steps go to any other hubs the embedder shares its agents with, and [`Routing::replies`] go back
//! down the connection itself. Fresh ids and timestamps come from the embedder's [`Stamps`].
//!
//! Everything here is `no_std` + `alloc`; the actix-web server under the `std` feature is one such embedder.

#[cfg(not(feature = "std"))]
use alloc::{
    format,
    string::{String, ToString as _},
    vec::Vec,
};
use core::net::IpAddr;

use bytes::Bytes;
use edge_toolkit::ws::{
    AgentSummary, ClientMessage, CloudEvent, ConnectStatus, MessageDeliveryStatus, MessageScope, ServerMessage,
    client_event_type,
};
use edge_toolkit::ws_server::{AgentRegistry, MessageRetention, PendingDirectMessage};
use tracing::{info, warn};

/// Outbound envelope written to an agent's websocket session.
///
/// `Json` is the normal path for protocol messages. `Text` and `Binary` carry payloads the server forwards verbatim --
/// used by the hub-style fallback that broadcasts unrecognised frames to every other connected agent.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum SessionMessage {
    Json(ServerMessage),
    Text(String),
    Binary(Bytes),
}

impl From<ServerMessage> for SessionMessage {
    fn from(value: ServerMessage) -> Self {
        Self::Json(value)
    }
}

/// Where the hub's fresh ids and receipt times come from.
pub trait Stamps {
    /// An id no other agent or message on the hub has: for an agent that names none, and for every message.
    fn new_id(&mut self) -> String;

    /// The current time, as an RFC 3339 timestamp.
    fn now(&mut self) -> String;
}

/// A step of a frame's routing that every other hub sharing this one's agents has to hear of.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Shared {
    /// `agent_id` is now reached through this hub. It precedes the replies, so an acknowledged agent is reachable.
    Joined { agent_id: String },
    /// `frame` from `from_agent_id` is for every agent, including those connected to other hubs.
    Broadcast {
        from_agent_id: String,
        frame: SessionMessage,
    },
}

/// What one frame asks the embedder to do.
#[derive(Debug)]
#[non_exhaustive]
pub struct Routing<S> {
    /// Messages for other agents, each with the session it goes to.
    pub deliveries: Vec<(S, SessionMessage)>,
    /// Steps other hubs must repeat, in order; a hub that shares its agents with none ignores them.
    pub shared: Vec<Shared>,
    /// Messages for the agent that sent the frame, in order.
    pub replies: Vec<ServerMessage>,
}

impl<S> Routing<S> {
    /// A routing whose only effect is `reply` to the sender.
    fn reply(reply: ServerMessage) -> Self {
        let mut routing = Self::default();
        routing.replies.push(reply);
        routing
    }
}

impl<S> Default for Routing<S> {
    fn default() -> Self {
        Self {
            deliveries: Vec::new(),
            shared: Vec::new(),
            replies: Vec::new(),
        }
    }
}

/// A delivery status for the sender of a message.
#[must_use]
pub fn status<D>(message_id: Option<String>, status: MessageDeliveryStatus, detail: D) -> ServerMessage
where
    D: Into<String>,
{
    ServerMessage::MessageStatus {
        message_id,
        status,
        detail: detail.into(),
    }
}

/// The refusal of a frame the hub cannot act on.
#[must_use]
pub fn invalid<D>(message_id: Option<String>, detail: D) -> ServerMessage
where
    D: Into<String>,
{
    ServerMessage::Invalid {
        message_id,
        detail: detail.into(),
    }
}

/// Why the hub will not act on a frame, as the agent that sent it is told.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Refusal(&'static str);

impl Refusal {
    /// The `et-invalid` reply that tells the agent.
    #[must_use]
    pub fn reply(self) -> ServerMessage {
        invalid(None, self.0)
    }
}

/// The direct message `pending` holds, as its recipient receives it.
#[must_use]
pub fn direct_message(pending: PendingDirectMessage) -> ServerMessage {
    ServerMessage::AgentMessage {
        message_id: pending.message_id,
        from_agent_id: pending.from_agent_id,
        scope: MessageScope::Direct,
        server_received_at: pending.server_received_at,
        message: pending.message,
    }
}

/// The receipt telling a sender that `recipient_agent_id` acknowledged its message `message_id`.
#[must_use]
pub fn receipt(recipient_agent_id: &str, message_id: String) -> ServerMessage {
    status(
        Some(message_id),
        MessageDeliveryStatus::Acknowledged,
        format!("agent {recipient_agent_id} acknowledged receipt"),
    )
}

/// Log an agent's `et-client-event`, the observability frame the hub records and does not answer.
///
/// An `et.video_cv.inference` event additionally gets its detected class, confidence and processing time logged as
/// fields of their own, so the camera modules' results read at a glance.
#[expect(
    clippy::cognitive_complexity,
    clippy::single_call_fn,
    reason = "the stateless client-event arm of the inbound dispatcher; its score is info! expansion"
)]
fn log_client_event(agent_id: &str, event: &CloudEvent) {
    let details = &event.data;
    if event.event_type == client_event_type("video_cv", "inference") {
        let detected_class = details
            .get("detected_class")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        let confidence = details
            .get("confidence")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or_default();
        let processed_at = details
            .get("processed_at")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        info!(
            "Video inference received from {}: class={} confidence={:.4} processed_at={}",
            agent_id, detected_class, confidence, processed_at
        );
    }
    info!(
        "Client event from {}: type={} source={} id={} time={} data={}",
        agent_id,
        event.event_type,
        event.source,
        event.id,
        event.time.to_rfc3339(),
        details
    );
}

/// One agent's connection to the hub: its identity in the registry, and the routing of what it sends.
///
/// `S` is the handle the embedder writes to the agent through; the registry keeps a clone of it for as long as the
/// agent holds its id.
#[derive(Debug)]
#[non_exhaustive]
pub struct Link<S> {
    agent_id: Option<String>,
    client_ip: IpAddr,
    session: S,
    /// Whether a direct message this agent sends replaces its earlier unacknowledged one to the same recipient.
    retention: MessageRetention,
}

impl<S: Clone + Send + 'static> Link<S> {
    /// A connection from `client_ip` that has not taken an agent id yet.
    #[must_use]
    pub const fn new(client_ip: IpAddr, session: S, retention: MessageRetention) -> Self {
        Self {
            agent_id: None,
            client_ip,
            session,
            retention,
        }
    }

    /// The agent id this connection holds, once it has one.
    #[must_use]
    pub fn agent_id(&self) -> Option<&str> {
        self.agent_id.as_deref()
    }

    /// The agent id, or `unassigned`, for log lines.
    #[must_use]
    pub fn current_agent_id(&self) -> &str {
        self.agent_id.as_deref().unwrap_or("unassigned")
    }

    /// The address the connection came from.
    #[must_use]
    pub const fn client_ip(&self) -> IpAddr {
        self.client_ip
    }

    /// The handle the registry holds for this connection.
    #[must_use]
    pub const fn session(&self) -> &S {
        &self.session
    }

    /// The retention of the direct messages this agent sends.
    #[must_use]
    pub const fn retention(&self) -> MessageRetention {
        self.retention
    }

    /// Take `requested_id`, or a fresh id, in `registry`.
    pub fn connect<T>(
        &mut self,
        registry: &AgentRegistry<S>,
        requested_id: Option<String>,
        stamps: &mut T,
    ) -> (String, ConnectStatus)
    where
        T: Stamps,
    {
        info!(
            "Connect message: requested_agent_id={:?} client_ip={}",
            requested_id, self.client_ip
        );
        let (assigned_id, status) = self.assign(registry, requested_id, stamps);
        info!(
            "Agent {} status {:?}connected from IP {}",
            assigned_id, status, self.client_ip
        );
        (assigned_id, status)
    }

    fn assign<T>(
        &mut self,
        registry: &AgentRegistry<S>,
        requested_id: Option<String>,
        stamps: &mut T,
    ) -> (String, ConnectStatus)
    where
        T: Stamps,
    {
        let (assigned_id, status) =
            registry.connect_agent(requested_id, stamps.new_id(), self.client_ip, self.session.clone());
        self.agent_id = Some(assigned_id.clone());
        (assigned_id, status)
    }

    /// The acknowledgement of a connect that took `agent_id` with `status`.
    #[must_use]
    pub fn connect_ack(&self, agent_id: String, status: ConnectStatus) -> ServerMessage {
        info!(
            "WebSocket connection ready for client {} with status {:?}",
            self.current_agent_id(),
            status
        );
        ServerMessage::ConnectAck { agent_id, status }
    }

    /// The direct messages waiting for this agent, as it receives them.
    #[must_use]
    pub fn pending_replies(&self, pending: Vec<PendingDirectMessage>) -> Vec<ServerMessage> {
        pending
            .into_iter()
            .map(|pending| {
                info!(
                    "Delivering pending message {} to agent {} from {}",
                    pending.message_id,
                    self.current_agent_id(),
                    pending.from_agent_id
                );
                direct_message(pending)
            })
            .collect()
    }

    /// The answer to `et-list-agents`, listing `agents`.
    #[must_use]
    pub fn listed(&self, agents: Vec<AgentSummary>) -> ServerMessage {
        info!(
            "Agent {} requested list_agents; returning {} agents",
            self.current_agent_id(),
            agents.len()
        );
        ServerMessage::ListAgentsResponse { agents }
    }

    /// The agent sending a direct message to `to_agent_id`, or the refusal when it may not.
    pub fn direct_sender(&self, to_agent_id: &str) -> Result<String, Refusal> {
        let Some(from_agent_id) = self.agent_id.clone() else {
            return Err(Refusal("agent must connect before sending messages"));
        };
        if from_agent_id == to_agent_id {
            return Err(Refusal("agent cannot send a direct message to itself"));
        }
        Ok(from_agent_id)
    }

    /// The agent acknowledging a message, or the refusal when it has not connected.
    pub fn acknowledger(&self) -> Result<String, Refusal> {
        self.agent_id
            .clone()
            .ok_or(Refusal("agent must connect before acknowledging messages"))
    }

    /// This connection's agent id, taking one on first use, with the routing that taking it needs.
    ///
    /// A client that never sends `et-connect` -- e.g. a frontend that speaks only its own protocol -- still joins the
    /// hub relay: its first unrecognised frame implicitly registers the session, so the frame is broadcast to the other
    /// agents and this client then receives the relayed replies. et-protocol agents send `et-connect` first, so they
    /// are already assigned by the time they relay and this is a no-op.
    fn ensure_assigned<T>(&mut self, registry: &AgentRegistry<S>, stamps: &mut T) -> (String, Routing<S>)
    where
        T: Stamps,
    {
        let mut routing = Routing::default();
        if let Some(id) = self.agent_id() {
            return (id.to_string(), routing);
        }
        let (id, _status) = self.assign(registry, None, stamps);
        routing.shared.push(Shared::Joined { agent_id: id.clone() });
        info!("Auto-registered relay client {} as agent {id}", self.client_ip);
        (id, routing)
    }

    /// Route one decoded frame through `registry`, the agents of this hub alone.
    pub fn route<T>(&mut self, registry: &AgentRegistry<S>, message: ClientMessage, stamps: &mut T) -> Routing<S>
    where
        T: Stamps,
    {
        match message {
            ClientMessage::Connect { agent_id } => {
                let (assigned_id, status) = self.connect(registry, agent_id, stamps);
                let mut routing = Routing::reply(self.connect_ack(assigned_id.clone(), status));
                routing
                    .replies
                    .extend(self.pending_replies(registry.pending_messages_for(&assigned_id)));
                routing.shared.push(Shared::Joined { agent_id: assigned_id });
                routing
            }
            ClientMessage::Alive { timestamp } => {
                info!("Alive message from client {} at {}", self.current_agent_id(), timestamp);
                Routing::reply(ServerMessage::Response {
                    message: format!("Alive message received at {}", stamps.now()),
                })
            }
            ClientMessage::ListAgents => Routing::reply(self.listed(registry.list_agents())),
            ClientMessage::SendAgentMessage { to_agent_id, message } => match self.direct_sender(&to_agent_id) {
                Ok(from_agent_id) => self.send_direct(registry, &from_agent_id, &to_agent_id, message, stamps),
                Err(refusal) => Routing::reply(refusal.reply()),
            },
            ClientMessage::BroadcastMessage { message } => self.broadcast(registry, message, stamps),
            ClientMessage::MessageAck { message_id } => match self.acknowledger() {
                Ok(recipient_agent_id) => Self::acknowledge(registry, &recipient_agent_id, message_id),
                Err(refusal) => Routing::reply(refusal.reply()),
            },
            ClientMessage::ClientEvent { event } => {
                log_client_event(self.current_agent_id(), &event);
                Routing::default()
            }
            ClientMessage::RelayText { content } => self.relay(registry, SessionMessage::Text(content), stamps),
            // A binary frame normally reaches the hub as one; a client that sends `et-relay-binary` as a text frame
            // has its payload relayed as a binary frame all the same.
            ClientMessage::RelayBinary { content } => {
                self.relay(registry, SessionMessage::Binary(Bytes::from(content)), stamps)
            }
        }
    }

    /// Route a binary frame, which is always a relay; the typed catalogue is JSON-only.
    pub fn route_binary<T>(&mut self, registry: &AgentRegistry<S>, bytes: Bytes, stamps: &mut T) -> Routing<S>
    where
        T: Stamps,
    {
        self.relay(registry, SessionMessage::Binary(bytes), stamps)
    }

    #[expect(
        clippy::cognitive_complexity,
        reason = "linear send/queue/unknown-recipient dispatch; splitting scatters the three status replies"
    )]
    fn send_direct<T>(
        &self,
        registry: &AgentRegistry<S>,
        from_agent_id: &str,
        to_agent_id: &str,
        message: serde_json::Value,
        stamps: &mut T,
    ) -> Routing<S>
    where
        T: Stamps,
    {
        let server_received_at = stamps.now();
        let Some((pending, recipient_session)) = registry.queue_direct(
            stamps.new_id(),
            from_agent_id,
            to_agent_id,
            server_received_at,
            message,
            self.retention,
        ) else {
            warn!("direct message target {to_agent_id} is not a connected agent");
            return Routing::reply(invalid(None, format!("unknown target agent {to_agent_id}")));
        };
        let message_id = pending.message_id.clone();

        let mut routing = Routing::default();
        if let Some(recipient) = recipient_session {
            info!(
                "Direct message {} delivered from {} to {}",
                message_id, from_agent_id, to_agent_id
            );
            routing
                .deliveries
                .push((recipient, SessionMessage::Json(direct_message(pending))));
            routing.replies.push(status(
                Some(message_id),
                MessageDeliveryStatus::Delivered,
                format!("message delivered to agent {to_agent_id}"),
            ));
        } else {
            info!(
                "Direct message {} queued from {} to disconnected agent {}",
                message_id, from_agent_id, to_agent_id
            );
            routing.replies.push(status(
                Some(message_id),
                MessageDeliveryStatus::Queued,
                format!("message queued for agent {to_agent_id}"),
            ));
        }
        routing
    }

    fn broadcast<T>(&self, registry: &AgentRegistry<S>, message: serde_json::Value, stamps: &mut T) -> Routing<S>
    where
        T: Stamps,
    {
        let Some(from_agent_id) = self.agent_id.clone() else {
            return Routing::reply(invalid(None, "agent must connect before broadcasting messages"));
        };
        let recipients = registry.connected_sessions(&from_agent_id);
        let message_id = stamps.new_id();
        let server_received_at = stamps.now();
        info!(
            "Broadcast message {} from {} to {} agent(s)",
            message_id,
            from_agent_id,
            recipients.len()
        );
        let broadcast = ServerMessage::AgentMessage {
            message_id: message_id.clone(),
            from_agent_id: from_agent_id.clone(),
            scope: MessageScope::Broadcast,
            server_received_at,
            message,
        };
        let mut routing = Routing::reply(status(
            Some(message_id),
            MessageDeliveryStatus::Broadcast,
            format!("broadcast sent to {} connected agents", recipients.len()),
        ));
        routing.deliveries = recipients
            .into_iter()
            .map(|(_, recipient)| (recipient, SessionMessage::Json(broadcast.clone())))
            .collect();
        routing.shared.push(Shared::Broadcast {
            from_agent_id,
            frame: SessionMessage::Json(broadcast),
        });
        routing
    }

    #[expect(
        clippy::cognitive_complexity,
        clippy::single_call_fn,
        reason = "the acknowledgement arm of route; its score is info!/warn! expansion"
    )]
    fn acknowledge(registry: &AgentRegistry<S>, recipient_agent_id: &str, message_id: String) -> Routing<S> {
        match registry.acknowledge_message(recipient_agent_id, &message_id) {
            Ok((message_id, sender_session, sender_agent_id)) => {
                info!(
                    "Agent {} acknowledged direct message {} from {}",
                    recipient_agent_id, message_id, sender_agent_id
                );
                let mut routing = Routing::reply(status(
                    Some(message_id.clone()),
                    MessageDeliveryStatus::Acknowledged,
                    "message acknowledged",
                ));
                if let Some(sender) = sender_session {
                    let receipt = receipt(recipient_agent_id, message_id);
                    routing.deliveries.push((sender, SessionMessage::Json(receipt)));
                }
                routing
            }
            Err(error) => {
                warn!("Invalid ack from {} for {}: {}", recipient_agent_id, message_id, error);
                Routing::reply(invalid(Some(message_id), error.to_string()))
            }
        }
    }

    /// Hub-style fallback: forward a raw frame to every connected agent except the sender.
    #[expect(
        clippy::cognitive_complexity,
        reason = "the score is info! expansion; the body branches once on the frame kind"
    )]
    fn relay<T>(&mut self, registry: &AgentRegistry<S>, frame: SessionMessage, stamps: &mut T) -> Routing<S>
    where
        T: Stamps,
    {
        let (from_agent_id, mut routing) = self.ensure_assigned(registry, stamps);
        let recipients = registry.connected_sessions(&from_agent_id);
        match &frame {
            SessionMessage::Binary(bytes) => info!(
                "Broadcasting unrecognised binary message ({} bytes) from {} to {} agent(s)",
                bytes.len(),
                from_agent_id,
                recipients.len()
            ),
            SessionMessage::Json(_) | SessionMessage::Text(_) => info!(
                "Broadcasting unrecognised text message from {} to {} agent(s)",
                from_agent_id,
                recipients.len()
            ),
        }
        routing.deliveries = recipients
            .into_iter()
            .map(|(_, recipient)| (recipient, frame.clone()))
            .collect();
        routing.shared.push(Shared::Broadcast { from_agent_id, frame });
        routing
    }

    /// Mark this connection's agent disconnected unless `is_current` says its session has been taken over.
    ///
    /// Returns whether it was marked; see [`AgentRegistry::mark_disconnected_if`].
    #[must_use]
    pub fn release(&self, registry: &AgentRegistry<S>, is_current: &dyn Fn(&S) -> bool) -> bool {
        self.agent_id
            .as_deref()
            .is_some_and(|agent_id| registry.mark_disconnected_if(agent_id, is_current))
    }
}
