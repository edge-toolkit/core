//! One agent's WebSocket connection: its registry identity, its inbound dispatch, and the read/write/heartbeat loop.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use actix_ws::{AggregatedMessage, AggregatedMessageStream, CloseCode, CloseReason, Session};
use bytes::Bytes;
use chrono::Utc;
use edge_toolkit::ws::{ClientMessage, ConnectStatus, MessageDeliveryStatus, MessageScope, ServerMessage};
use edge_toolkit::ws_server::{MessageRetention, PendingDirectMessage};
use et_hub_backplane::{Backplane, BackplaneError, Frame, NatsBackplane, Presence};
use futures_util::StreamExt as _;
use opentelemetry::global;
use opentelemetry::trace::{Span, Tracer as _};
use tokio::sync::mpsc::UnboundedReceiver;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::{
    ACTIVE_CONNECTIONS, AgentSession, HEARTBEAT_INTERVAL, MESSAGES_RECEIVED, SessionMessage, WsAgentRegistry,
    log_client_event,
};

pub(crate) struct Connection {
    agent_id: Option<String>,
    last_activity: Instant,
    client_ip: IpAddr,
    registry: WsAgentRegistry,
    /// Where agents on other hubs are found; in NATS mode it, not `registry`, decides where a message goes.
    backplane: Backplane,
    session: Session,
    outbox: AgentSession,
    /// Idle timeout for this connection, or `None` to never time out.
    idle_timeout: Option<Duration>,
    /// Whether a direct message this agent sends replaces its earlier unacknowledged one to the same recipient.
    retention: MessageRetention,
}

impl Connection {
    #[expect(clippy::single_call_fn, reason = "inherent constructor; used once by ws_handler")]
    pub(crate) fn new(
        registry: WsAgentRegistry,
        backplane: Backplane,
        client_ip: IpAddr,
        session: Session,
        outbox: AgentSession,
        idle_timeout: Option<Duration>,
        retention: MessageRetention,
    ) -> Self {
        info!("New WebSocket connection for client IP {}", client_ip);
        Self {
            agent_id: None,
            last_activity: Instant::now(),
            client_ip,
            registry,
            backplane,
            session,
            outbox,
            idle_timeout,
            retention,
        }
    }

    fn current_agent_id(&self) -> &str {
        self.agent_id.as_deref().unwrap_or("unassigned")
    }

    fn assigned_agent_id(&self) -> Option<&str> {
        self.agent_id.as_deref()
    }

    fn mark_activity(&mut self) {
        self.last_activity = Instant::now();
    }

    fn assign_or_reconnect_agent(&mut self, requested_id: Option<String>) -> (String, ConnectStatus) {
        let new_id = Uuid::now_v7().to_string();
        let (assigned_id, status) =
            self.registry
                .connect_agent(requested_id, new_id, self.client_ip, self.outbox.clone());
        self.agent_id = Some(assigned_id.clone());
        (assigned_id, status)
    }

    /// This connection's agent id, auto-registering one on first use.
    ///
    /// A client that never sends `et-connect` -- e.g. a frontend that speaks only its own protocol -- still joins the
    /// hub relay: its first unrecognised frame implicitly registers the session, so the frame is broadcast to the other
    /// agents and this client then receives the relayed replies. et-protocol agents send `et-connect` first, so they
    /// are already assigned by the time they relay and this is a no-op.
    async fn ensure_assigned_agent(&mut self) -> String {
        if let Some(id) = self.assigned_agent_id() {
            return id.to_string();
        }
        let (id, _status) = self.assign_or_reconnect_agent(None);
        self.announce(&id).await;
        info!("Auto-registered relay client {} as agent {id}", self.client_ip);
        id
    }

    /// Place `agent_id` on this hub in the shared directory, taking it over from any hub that held it before.
    ///
    /// A failure is logged rather than refused: the agent is still served by this hub, only unreachable from others.
    async fn announce(&self, agent_id: &str) {
        if let Backplane::Nats(nats) = &self.backplane
            && let Err(error) = nats.announce(agent_id, self.client_ip).await
        {
            error!("Failed to announce agent {agent_id} on the backplane: {error}");
        }
    }

    /// Everything waiting for `agent_id`: in this process's registry, or in the shared mailbox in NATS mode.
    async fn pending_messages(&self, agent_id: &str) -> Vec<PendingDirectMessage> {
        match &self.backplane {
            Backplane::Local => self.registry.pending_messages_for(agent_id),
            Backplane::Nats(nats) => nats.pending_for(agent_id).await.unwrap_or_else(|error| {
                error!("Failed to read the backplane mailbox of {agent_id}: {error}");
                Vec::new()
            }),
        }
    }

    /// Hand `message` to `to_agent_id`, whose socket the directory places on hub `hub_id`.
    ///
    /// The local session is written directly when that hub is this one; anything else crosses the backplane.
    async fn deliver_via_backplane(
        &self,
        nats: &NatsBackplane,
        hub_id: &str,
        to_agent_id: &str,
        message: ServerMessage,
    ) -> Result<(), BackplaneError> {
        if hub_id == nats.hub_id() {
            if let Some(session) = self.registry.agent_session(to_agent_id) {
                let _sent = session.send(SessionMessage::Json(message));
            }
            return Ok(());
        }
        let text = serde_json::to_string(&message)?;
        nats.send_to_hub(hub_id, to_agent_id, Frame::Text(text)).await
    }

    /// Send `frame` from `from_agent_id` to the agents of every other hub; a no-op for a local hub.
    async fn broadcast_to_other_hubs(&self, from_agent_id: &str, frame: Frame) {
        if let Backplane::Nats(nats) = &self.backplane
            && let Err(error) = nats.broadcast(from_agent_id, frame).await
        {
            warn!("Failed to broadcast from {from_agent_id} to the other hubs: {error}");
        }
    }

    /// The direct-message path when a NATS backplane decides where the recipient is.
    ///
    /// Every message lands in the shared mailbox first, so it survives the recipient reconnecting to another hub before
    /// it acknowledges, whichever hub it is delivered through.
    #[expect(
        clippy::cognitive_complexity,
        reason = "the same three status replies as the local path, plus the backplane failure reply"
    )]
    async fn send_direct_via_backplane(
        &mut self,
        nats: &NatsBackplane,
        from_agent_id: String,
        to_agent_id: String,
        message: serde_json::Value,
    ) {
        let destination = match nats.presence(&to_agent_id).await {
            Ok(Presence::Unknown) => {
                warn!("direct message target {to_agent_id} is not a known agent");
                self.send_invalid(None, format!("unknown target agent {to_agent_id}"))
                    .await;
                return;
            }
            Ok(presence) => presence,
            Err(error) => {
                error!("Backplane lookup of {to_agent_id} failed: {error}");
                self.send_backplane_unavailable(&error).await;
                return;
            }
        };
        let pending = PendingDirectMessage::new(
            Uuid::now_v7().to_string(),
            from_agent_id,
            Utc::now().to_rfc3339(),
            message,
        );
        if let Err(error) = nats.queue(&to_agent_id, pending.clone(), self.retention).await {
            error!("Backplane mailbox write for {to_agent_id} failed: {error}");
            self.send_backplane_unavailable(&error).await;
            return;
        }
        // A recipient that connected between the first lookup and the write has already read its mailbox, so look
        // again. It announces before it reads, so a second look that still says disconnected means its read is yet to
        // come and will find this message.
        let destination = match destination {
            Presence::Connected { .. } => destination,
            Presence::Disconnected | Presence::Unknown => match nats.presence(&to_agent_id).await {
                Ok(presence) => presence,
                Err(error) => {
                    warn!("Backplane re-check of {to_agent_id} failed, leaving the message queued: {error}");
                    destination
                }
            },
        };

        if let Presence::Connected { hub_id, .. } = destination {
            self.deliver_live(nats, &hub_id, &to_agent_id, pending).await;
            return;
        }
        info!(
            "Direct message {} queued for disconnected agent {to_agent_id}",
            pending.message_id
        );
        self.send_status(
            Some(pending.message_id),
            MessageDeliveryStatus::Queued,
            format!("message queued for agent {to_agent_id}"),
        )
        .await;
    }

    /// Tell the client its request failed because the backplane could not be reached.
    async fn send_backplane_unavailable(&mut self, error: &BackplaneError) {
        self.send_invalid(None, format!("hub backplane unavailable: {error}"))
            .await;
    }

    /// Deliver `pending`, already in the mailbox, to `to_agent_id` on hub `hub_id`, and report it delivered.
    #[expect(
        clippy::cognitive_complexity,
        reason = "the score is info!/warn! expansion; the body branches once"
    )]
    async fn deliver_live(
        &mut self,
        nats: &NatsBackplane,
        hub_id: &str,
        to_agent_id: &str,
        pending: PendingDirectMessage,
    ) {
        let message_id = pending.message_id.clone();
        let agent_message = ServerMessage::AgentMessage {
            message_id: message_id.clone(),
            from_agent_id: pending.from_agent_id,
            scope: MessageScope::Direct,
            server_received_at: pending.server_received_at,
            message: pending.message,
        };
        let (status, detail) = match self
            .deliver_via_backplane(nats, hub_id, to_agent_id, agent_message)
            .await
        {
            Ok(()) => {
                info!("Direct message {message_id} delivered to {to_agent_id} on hub {hub_id}");
                (MessageDeliveryStatus::Delivered, "delivered to")
            }
            // Already in the mailbox, so queued is the truth: the recipient gets it on its next connect.
            Err(error) => {
                warn!("Live delivery of {message_id} to {to_agent_id} on hub {hub_id} failed: {error}");
                (MessageDeliveryStatus::Queued, "queued for")
            }
        };
        self.send_status(
            Some(message_id),
            status,
            format!("message {detail} agent {to_agent_id}"),
        )
        .await;
    }

    /// The ack path when the message sits in the shared mailbox, telling its sender wherever that sender is now.
    #[expect(
        clippy::cognitive_complexity,
        reason = "the score is info!/warn! expansion; the body branches once"
    )]
    async fn acknowledge_via_backplane(
        &mut self,
        nats: &NatsBackplane,
        recipient_agent_id: String,
        message_id: String,
    ) {
        let sender_agent_id = match nats.acknowledge(&recipient_agent_id, &message_id).await {
            Ok(sender_agent_id) => sender_agent_id,
            Err(error) => {
                warn!("Invalid ack from {recipient_agent_id} for {message_id}: {error}");
                self.send_invalid(Some(message_id), error.to_string()).await;
                return;
            }
        };
        info!("Agent {recipient_agent_id} acknowledged direct message {message_id} from {sender_agent_id}");
        self.send_status(
            Some(message_id.clone()),
            MessageDeliveryStatus::Acknowledged,
            "message acknowledged",
        )
        .await;
        self.send_receipt_via_backplane(nats, &recipient_agent_id, &sender_agent_id, message_id)
            .await;
    }

    /// Tell `sender_agent_id`, on whichever hub holds it, that `recipient_agent_id` acknowledged `message_id`.
    async fn send_receipt_via_backplane(
        &self,
        nats: &NatsBackplane,
        recipient_agent_id: &str,
        sender_agent_id: &str,
        message_id: String,
    ) {
        let receipt = ServerMessage::MessageStatus {
            message_id: Some(message_id),
            status: MessageDeliveryStatus::Acknowledged,
            detail: format!("agent {recipient_agent_id} acknowledged receipt"),
        };
        let delivered = match nats.presence(sender_agent_id).await {
            Ok(Presence::Connected { hub_id, .. }) => {
                self.deliver_via_backplane(nats, &hub_id, sender_agent_id, receipt)
                    .await
            }
            Ok(Presence::Disconnected | Presence::Unknown) => Ok(()),
            Err(error) => Err(error),
        };
        if let Err(error) = delivered {
            warn!("Failed to tell {sender_agent_id} its message was acknowledged: {error}");
        }
    }

    #[expect(
        clippy::cognitive_complexity,
        reason = "the score is warn!/error! expansion; the body branches twice"
    )]
    async fn send_json(&mut self, response: &ServerMessage) {
        match serde_json::to_string(response) {
            Ok(json) => {
                if let Err(err) = self.session.text(json).await {
                    warn!("Failed to send message to {}: {:?}", self.current_agent_id(), err);
                } else {
                    let tracer = global::tracer("ws-server");
                    let mut sent_span = tracer.start("ws.message.sent");
                    sent_span.end();
                }
            }
            Err(error) => {
                error!("Failed to serialize websocket response: {}", error);
            }
        }
    }

    async fn send_text(&mut self, text: String) {
        if let Err(err) = self.session.text(text).await {
            warn!("Failed to forward text to {}: {:?}", self.current_agent_id(), err);
        }
    }

    async fn send_binary(&mut self, bytes: Bytes) {
        if let Err(err) = self.session.binary(bytes).await {
            warn!("Failed to forward binary to {}: {:?}", self.current_agent_id(), err);
        }
    }

    async fn send_status(
        &mut self,
        message_id: Option<String>,
        status: MessageDeliveryStatus,
        detail: impl Into<String>,
    ) {
        self.send_json(&ServerMessage::MessageStatus {
            message_id,
            status,
            detail: detail.into(),
        })
        .await;
    }

    async fn send_invalid(&mut self, message_id: Option<String>, detail: impl Into<String>) {
        self.send_json(&ServerMessage::Invalid {
            message_id,
            detail: detail.into(),
        })
        .await;
    }

    async fn deliver_pending_messages(&mut self) {
        let Some(agent_id) = self.assigned_agent_id().map(str::to_string) else {
            return;
        };
        for pending in self.pending_messages(&agent_id).await {
            info!(
                "Delivering pending message {} to agent {} from {}",
                pending.message_id, agent_id, pending.from_agent_id
            );
            self.send_json(&ServerMessage::AgentMessage {
                message_id: pending.message_id,
                from_agent_id: pending.from_agent_id,
                scope: MessageScope::Direct,
                server_received_at: pending.server_received_at,
                message: pending.message,
            })
            .await;
        }
    }

    #[expect(
        clippy::cognitive_complexity,
        reason = "linear send/queue/unknown-recipient dispatch; splitting scatters the three status replies"
    )]
    async fn handle_send_direct(
        &mut self,
        span: &mut impl Span,
        from_agent_id: String,
        to_agent_id: String,
        message: serde_json::Value,
    ) {
        if let Backplane::Nats(nats) = &self.backplane {
            let nats = nats.clone();
            self.send_direct_via_backplane(&nats, from_agent_id, to_agent_id, message)
                .await;
            span.end();
            return;
        }
        let server_received_at = Utc::now().to_rfc3339();
        let Some((pending, recipient_session)) = self.registry.queue_direct(
            Uuid::now_v7().to_string(),
            &from_agent_id,
            &to_agent_id,
            server_received_at,
            message,
            self.retention,
        ) else {
            warn!("direct message target {to_agent_id} is not a connected agent");
            self.send_invalid(None, format!("unknown target agent {to_agent_id}"))
                .await;
            span.end();
            return;
        };
        let message_id = pending.message_id.clone();

        if let Some(recipient) = recipient_session {
            info!(
                "Direct message {} delivered from {} to {}",
                message_id, from_agent_id, to_agent_id
            );
            let _sent = recipient.send(SessionMessage::Json(ServerMessage::AgentMessage {
                message_id: message_id.clone(),
                from_agent_id,
                scope: MessageScope::Direct,
                server_received_at: pending.server_received_at,
                message: pending.message,
            }));
            self.send_status(
                Some(message_id),
                MessageDeliveryStatus::Delivered,
                format!("message delivered to agent {to_agent_id}"),
            )
            .await;
        } else {
            info!(
                "Direct message {} queued from {} to disconnected agent {}",
                message_id, from_agent_id, to_agent_id
            );
            self.send_status(
                Some(message_id),
                MessageDeliveryStatus::Queued,
                format!("message queued for agent {to_agent_id}"),
            )
            .await;
        }
        span.end();
    }

    /// Hub-style fallback: forward raw text to every connected agent except the sender.
    ///
    /// Used by the `RelayText` / `RelayBinary` arms of the inbound dispatcher when a frame doesn't carry our `et-*`
    /// tag.
    fn broadcast_raw_text(&self, from_agent_id: &str, text: &str) {
        let recipients = self.registry.connected_sessions(from_agent_id);
        info!(
            "Broadcasting unrecognised text message from {} to {} agent(s)",
            from_agent_id,
            recipients.len()
        );
        for (_, recipient) in recipients {
            let _sent = recipient.send(SessionMessage::Text(text.to_string()));
        }
    }

    /// Hub-style fallback for binary frames -- same shape as the text path.
    fn broadcast_raw_binary(&self, from_agent_id: &str, bytes: &Bytes) {
        let recipients = self.registry.connected_sessions(from_agent_id);
        info!(
            "Broadcasting unrecognised binary message ({} bytes) from {} to {} agent(s)",
            bytes.len(),
            from_agent_id,
            recipients.len()
        );
        for (_, recipient) in recipients {
            let _sent = recipient.send(SessionMessage::Binary(bytes.clone()));
        }
    }

    /// Returns `false` when the connection should terminate.
    #[expect(
        clippy::cognitive_complexity,
        clippy::too_many_lines,
        reason = "single dispatcher for inbound ClientMessage variants; splitting it scatters handlers into trivial fns"
    )]
    // skipcq: RS-R1000 -- dispatcher cyclomatic complexity is inherent to the ClientMessage match; not splittable
    async fn handle_inbound(&mut self, msg: AggregatedMessage) -> bool {
        MESSAGES_RECEIVED.add(1, &[]);
        match msg {
            AggregatedMessage::Ping(ping) => {
                self.mark_activity();
                let _pong: Result<(), actix_ws::Closed> = self.session.pong(&ping).await;
            }
            AggregatedMessage::Pong(_) => {
                self.mark_activity();
            }
            AggregatedMessage::Binary(bytes) => {
                self.mark_activity();
                let from_agent_id = self.ensure_assigned_agent().await;
                self.broadcast_raw_binary(&from_agent_id, &bytes);
                self.broadcast_to_other_hubs(&from_agent_id, Frame::Binary(bytes)).await;
            }
            AggregatedMessage::Close(reason) => {
                self.mark_activity();
                info!(
                    "WebSocket close request from client: {} reason: {:?}",
                    self.current_agent_id(),
                    reason
                );
                let tracer = global::tracer("ws-server");
                let mut span = tracer.start("ws.disconnect");
                span.end();
                let _closed: Result<(), actix_ws::Closed> = self.session.clone().close(reason).await;
                return false;
            }
            AggregatedMessage::Text(text) => {
                self.mark_activity();
                let tracer = global::tracer("ws-server");
                let mut span = tracer.start("ws.message.received");
                info!("Received message from client {}: {:?}", self.current_agent_id(), text);

                match ClientMessage::from_text_frame(&text) {
                    Err(err) => {
                        warn!("Decode error for et-* frame from {}: {}", self.current_agent_id(), err);
                    }
                    Ok(msg) => match msg {
                        ClientMessage::Connect { agent_id } => {
                            let requested_id = agent_id.clone();
                            info!(
                                "Connect message: requested_agent_id={:?} client_ip={}",
                                requested_id, self.client_ip
                            );
                            let (assigned_id, status) = self.assign_or_reconnect_agent(agent_id);
                            // Before the ack, so an agent that has been acked is already reachable from every hub.
                            self.announce(&assigned_id).await;
                            info!(
                                "Agent {} status {:?}connected from IP {}",
                                assigned_id, status, self.client_ip
                            );
                            self.send_json(&ServerMessage::ConnectAck {
                                agent_id: assigned_id,
                                status: status.clone(),
                            })
                            .await;
                            info!(
                                "WebSocket connection ready for client {} with status {:?}",
                                self.current_agent_id(),
                                status
                            );
                            self.deliver_pending_messages().await;
                        }
                        ClientMessage::Alive { timestamp } => {
                            info!("Alive message from client {} at {}", self.current_agent_id(), timestamp);
                            self.send_json(&ServerMessage::Response {
                                message: format!("Alive message received at {}", Utc::now().to_rfc3339()),
                            })
                            .await;
                        }
                        ClientMessage::ListAgents => {
                            let agents = match &self.backplane {
                                Backplane::Local => self.registry.list_agents(),
                                Backplane::Nats(nats) => match nats.list_agents().await {
                                    Ok(agents) => agents,
                                    Err(error) => {
                                        error!("Backplane directory listing failed: {error}");
                                        self.send_backplane_unavailable(&error).await;
                                        span.end();
                                        return true;
                                    }
                                },
                            };
                            info!(
                                "Agent {} requested list_agents; returning {} agents",
                                self.current_agent_id(),
                                agents.len()
                            );
                            self.send_json(&ServerMessage::ListAgentsResponse { agents }).await;
                        }
                        ClientMessage::SendAgentMessage { to_agent_id, message } => {
                            let Some(from_agent_id) = self.assigned_agent_id().map(str::to_string) else {
                                self.send_invalid(None, "agent must connect before sending messages")
                                    .await;
                                span.end();
                                return true;
                            };

                            if from_agent_id == to_agent_id {
                                self.send_invalid(None, "agent cannot send a direct message to itself")
                                    .await;
                                span.end();
                                return true;
                            }

                            // Unknown / departed recipients are handled by handle_send_direct's queue miss below -- a
                            // single place that answers Invalid -- so there is no pre-check here.
                            self.handle_send_direct(&mut span, from_agent_id, to_agent_id, message)
                                .await;
                            return true;
                        }
                        ClientMessage::BroadcastMessage { message } => {
                            let Some(from_agent_id) = self.assigned_agent_id().map(str::to_string) else {
                                self.send_invalid(None, "agent must connect before broadcasting messages")
                                    .await;
                                span.end();
                                return true;
                            };

                            let recipients = self.registry.connected_sessions(&from_agent_id);
                            let message_id = Uuid::now_v7().to_string();
                            let server_received_at = Utc::now().to_rfc3339();
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
                            for (_, recipient) in &recipients {
                                let _sent = recipient.send(SessionMessage::Json(broadcast.clone()));
                            }
                            match serde_json::to_string(&broadcast) {
                                Ok(text) => self.broadcast_to_other_hubs(&from_agent_id, Frame::Text(text)).await,
                                Err(error) => error!("Failed to serialize broadcast {message_id}: {error}"),
                            }
                            self.send_status(
                                Some(message_id),
                                MessageDeliveryStatus::Broadcast,
                                format!("broadcast sent to {} connected agents", recipients.len()),
                            )
                            .await;
                        }
                        ClientMessage::MessageAck { message_id } => {
                            let Some(recipient_agent_id) = self.assigned_agent_id().map(str::to_string) else {
                                self.send_invalid(None, "agent must connect before acknowledging messages")
                                    .await;
                                span.end();
                                return true;
                            };

                            if let Backplane::Nats(nats) = &self.backplane {
                                let nats = nats.clone();
                                self.acknowledge_via_backplane(&nats, recipient_agent_id, message_id)
                                    .await;
                                span.end();
                                return true;
                            }
                            match self.registry.acknowledge_message(&recipient_agent_id, &message_id) {
                                Ok((message_id, sender_session, sender_agent_id)) => {
                                    info!(
                                        "Agent {} acknowledged direct message {} from {}",
                                        recipient_agent_id, message_id, sender_agent_id
                                    );
                                    self.send_status(
                                        Some(message_id.clone()),
                                        MessageDeliveryStatus::Acknowledged,
                                        "message acknowledged",
                                    )
                                    .await;
                                    if let Some(sender) = sender_session {
                                        let _sent = sender.send(SessionMessage::Json(ServerMessage::MessageStatus {
                                            message_id: Some(message_id),
                                            status: MessageDeliveryStatus::Acknowledged,
                                            detail: format!("agent {recipient_agent_id} acknowledged receipt"),
                                        }));
                                    }
                                }
                                Err(error) => {
                                    warn!("Invalid ack from {} for {}: {}", recipient_agent_id, message_id, error);
                                    self.send_invalid(Some(message_id), error.to_string()).await;
                                }
                            }
                        }
                        ClientMessage::ClientEvent { event } => log_client_event(self.current_agent_id(), &event),
                        ClientMessage::RelayText { content } => {
                            let from_agent_id = self.ensure_assigned_agent().await;
                            self.broadcast_raw_text(&from_agent_id, &content);
                            self.broadcast_to_other_hubs(&from_agent_id, Frame::Text(content)).await;
                        }
                        ClientMessage::RelayBinary { content } => {
                            // A binary tungstenite frame is dispatched by the outer `AggregatedMessage::Binary` arm. If
                            // a client explicitly sends `{"type":"et-relay-binary",...}` as a text frame, honour it by
                            // relaying the payload as a binary frame.
                            let from_agent_id = self.ensure_assigned_agent().await;
                            let bytes = Bytes::from(content);
                            self.broadcast_raw_binary(&from_agent_id, &bytes);
                            self.broadcast_to_other_hubs(&from_agent_id, Frame::Binary(bytes)).await;
                        }
                    },
                }
                span.end();
            }
        }
        true
    }

    /// Close the session when it has been idle past its timeout, reporting whether it did.
    async fn close_if_idle(&self) -> bool {
        let Some(timeout) = self.idle_timeout else {
            return false;
        };
        let idle_for = Instant::now().saturating_duration_since(self.last_activity);
        if idle_for <= timeout {
            return false;
        }
        warn!(
            "WebSocket connection timed out for client {} after {:?} of inactivity",
            self.current_agent_id(),
            idle_for
        );
        let reason = CloseReason {
            code: CloseCode::Policy,
            description: Some(format!("connection timed out after {timeout:?} of inactivity")),
        };
        let _closed: Result<(), actix_ws::Closed> = self.session.clone().close(Some(reason)).await;
        true
    }

    /// Close the session when its client address has been blocked since it connected, reporting whether it did.
    async fn close_if_blocked(&self) -> bool {
        if !self.registry.blocklist.is_blocked(self.client_ip) {
            return false;
        }
        warn!(
            "Closing WebSocket connection for client {} from blocked IP {}",
            self.current_agent_id(),
            self.client_ip
        );
        let reason = CloseReason {
            code: CloseCode::Policy,
            description: Some("client address blocked".to_owned()),
        };
        let _closed: Result<(), actix_ws::Closed> = self.session.clone().close(Some(reason)).await;
        true
    }

    #[expect(
        clippy::cognitive_complexity,
        clippy::future_not_send,
        clippy::integer_division_remainder_used,
        reason = "actix-ws AggregatedMessageStream is Rc-backed and !Send; tokio::select! macro uses % internally"
    )]
    pub(crate) async fn run(
        mut self,
        mut stream: AggregatedMessageStream,
        mut outbound: UnboundedReceiver<SessionMessage>,
    ) {
        let tracer = global::tracer("ws-server");
        let mut connect_span = tracer.start("ws.connect");
        info!(
            "WebSocket connection established for client IP {} with agent {}",
            self.client_ip,
            self.current_agent_id()
        );
        connect_span.end();
        ACTIVE_CONNECTIONS.add(1, &[]);

        let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                msg = stream.next() => {
                    match msg {
                        // A frame is refused once its address is blocked, not only at the next heartbeat, so nothing
                        // an evicted agent sends lands after the eviction dropped what it had queued.
                        Some(Ok(msg)) => {
                            if self.close_if_blocked().await || !self.handle_inbound(msg).await {
                                break;
                            }
                        }
                        Some(Err(e)) => {
                            error!("WebSocket error for client {}: {:?}", self.current_agent_id(), e);
                            let mut err_span = tracer.start("ws.error");
                            err_span.end();
                            break;
                        }
                        None => break,
                    }
                }
                Some(envelope) = outbound.recv() => {
                    match envelope {
                        SessionMessage::Json(message) => self.send_json(&message).await,
                        SessionMessage::Text(text) => self.send_text(text).await,
                        SessionMessage::Binary(bytes) => self.send_binary(bytes).await,
                    }
                }
                _ = heartbeat.tick() => {
                    if self.close_if_blocked().await || self.close_if_idle().await {
                        break;
                    }
                }
            }
        }

        ACTIVE_CONNECTIONS.add(-1, &[]);
        self.release_identity().await;
    }

    /// Mark this connection's agent disconnected, here and in the shared directory, unless it has been taken over.
    #[expect(
        clippy::cognitive_complexity,
        reason = "the score is info!/warn! expansion; the body branches three times"
    )]
    async fn release_identity(&self) {
        let Some(agent_id) = self.agent_id.as_deref() else {
            info!(
                "WebSocket connection closed before agent assignment for client IP {}",
                self.client_ip
            );
            return;
        };
        // Only this connection's own session: another may have taken the id over since, and is still live.
        let was_current = self
            .registry
            .mark_disconnected_if(agent_id, &|session| session.same_channel(&self.outbox));
        if was_current
            && let Backplane::Nats(nats) = &self.backplane
            && let Err(error) = nats.withdraw(agent_id).await
        {
            warn!("Failed to withdraw agent {agent_id} from the backplane: {error}");
        }
        info!("Agent {} disconnected; last known IP {}", agent_id, self.client_ip);
    }
}
