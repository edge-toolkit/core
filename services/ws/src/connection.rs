//! One agent's WebSocket connection: the read/write/heartbeat loop around the routing its [`Link`] decides.

use std::time::{Duration, Instant};

use actix_ws::{AggregatedMessage, AggregatedMessageStream, CloseCode, CloseReason, Session};
use bytes::Bytes;
#[cfg(feature = "nats")]
use edge_toolkit::ws::MessageDeliveryStatus;
use edge_toolkit::ws::{ClientMessage, ServerMessage};
#[cfg(feature = "nats")]
use edge_toolkit::ws_server::PendingDirectMessage;
#[cfg(feature = "nats")]
use et_hub_backplane::{Backplane, BackplaneError, Frame, NatsBackplane, Presence};
use futures_util::StreamExt as _;
use tokio::sync::mpsc::UnboundedReceiver;
use tracing::{error, info, warn};

#[cfg(feature = "nats")]
use crate::hub::{self, Shared, Stamps as _};
use crate::hub::{Link, Routing, SessionMessage};
use crate::server::{AgentSession, HEARTBEAT_INTERVAL, SystemStamps, WsAgentRegistry};
use crate::telemetry::{self, HubSpan};

pub(crate) struct Connection {
    link: Link<AgentSession>,
    last_activity: Instant,
    registry: WsAgentRegistry,
    /// Where agents on other hubs are found; in NATS mode it, not `registry`, decides where a message goes.
    #[cfg(feature = "nats")]
    backplane: Backplane,
    session: Session,
    /// Idle timeout for this connection, or `None` to never time out.
    idle_timeout: Option<Duration>,
}

impl Connection {
    #[expect(clippy::single_call_fn, reason = "inherent constructor; used once by ws_handler")]
    pub(crate) fn new(
        registry: WsAgentRegistry,
        link: Link<AgentSession>,
        session: Session,
        idle_timeout: Option<Duration>,
    ) -> Self {
        info!("New WebSocket connection for client IP {}", link.client_ip());
        Self {
            link,
            last_activity: Instant::now(),
            registry,
            #[cfg(feature = "nats")]
            backplane: Backplane::Local,
            session,
            idle_timeout,
        }
    }

    /// Share this connection's agents through `backplane` rather than keeping them to this hub.
    #[cfg(feature = "nats")]
    pub(crate) fn with_backplane(mut self, backplane: Backplane) -> Self {
        self.backplane = backplane;
        self
    }

    fn mark_activity(&mut self) {
        self.last_activity = Instant::now();
    }

    /// Carry out `routing`: other agents' messages, then the steps other hubs repeat, then the replies.
    async fn apply(&mut self, routing: Routing<AgentSession>) {
        for (session, message) in routing.deliveries {
            let _sent = session.send(message);
        }
        #[cfg(feature = "nats")]
        for shared in routing.shared {
            self.share(shared).await;
        }
        for reply in &routing.replies {
            self.send_json(reply).await;
        }
    }

    /// Repeat `shared` on every other hub of the backplane; a no-op for a local hub.
    #[cfg(feature = "nats")]
    #[expect(
        clippy::cognitive_complexity,
        reason = "the score is error!/warn! expansion; the body branches three times"
    )]
    async fn share(&self, shared: Shared) {
        let Backplane::Nats(nats) = &self.backplane else {
            return;
        };
        match shared {
            Shared::Joined { agent_id } => self.announce(nats, &agent_id).await,
            Shared::Broadcast { from_agent_id, frame } => match backplane_frame(frame) {
                Ok(frame) => {
                    if let Err(error) = nats.broadcast(&from_agent_id, frame).await {
                        warn!("Failed to broadcast from {from_agent_id} to the other hubs: {error}");
                    }
                }
                Err(error) => error!("Failed to serialize broadcast from {from_agent_id}: {error}"),
            },
        }
    }

    /// Place `agent_id` on this hub in the shared directory, taking it over from any hub that held it before.
    ///
    /// A failure is logged rather than refused: the agent is still served by this hub, only unreachable from others.
    #[cfg(feature = "nats")]
    async fn announce(&self, nats: &NatsBackplane, agent_id: &str) {
        if let Err(error) = nats.announce(agent_id, self.link.client_ip()).await {
            error!("Failed to announce agent {agent_id} on the backplane: {error}");
        }
    }

    /// Route `message` on a hub that shares its agents through `nats`.
    ///
    /// A connect reads the shared mailbox, a listing reads the shared directory, and direct messages and their
    /// acknowledgements go through the shared mailbox; everything else routes as on a local hub.
    #[cfg(feature = "nats")]
    #[expect(
        clippy::cognitive_complexity,
        reason = "one arm per frame the backplane routes; the score is error! expansion"
    )]
    async fn route_via_backplane(&mut self, nats: &NatsBackplane, message: ClientMessage) {
        match message {
            ClientMessage::Connect { agent_id } => {
                let (assigned_id, status) = self.link.connect(&self.registry, agent_id, &mut SystemStamps);
                // Before the ack, so an agent that has been acked is already reachable from every hub.
                self.announce(nats, &assigned_id).await;
                let ack = self.link.connect_ack(assigned_id.clone(), status);
                self.send_json(&ack).await;
                let pending = nats.pending_for(&assigned_id).await.unwrap_or_else(|error| {
                    error!("Failed to read the backplane mailbox of {assigned_id}: {error}");
                    Vec::new()
                });
                for reply in self.link.pending_replies(pending) {
                    self.send_json(&reply).await;
                }
            }
            ClientMessage::ListAgents => match nats.list_agents().await {
                Ok(agents) => {
                    let listed = self.link.listed(agents);
                    self.send_json(&listed).await;
                }
                Err(error) => {
                    error!("Backplane directory listing failed: {error}");
                    self.send_backplane_unavailable(&error).await;
                }
            },
            ClientMessage::SendAgentMessage { to_agent_id, message } => match self.link.direct_sender(&to_agent_id) {
                Ok(from_agent_id) => {
                    self.send_direct_via_backplane(nats, from_agent_id, to_agent_id, message)
                        .await;
                }
                Err(refusal) => self.send_json(&refusal.reply()).await,
            },
            ClientMessage::MessageAck { message_id } => match self.link.acknowledger() {
                Ok(recipient_agent_id) => {
                    self.acknowledge_via_backplane(nats, recipient_agent_id, message_id)
                        .await;
                }
                Err(refusal) => self.send_json(&refusal.reply()).await,
            },
            other @ (ClientMessage::Alive { .. }
            | ClientMessage::BroadcastMessage { .. }
            | ClientMessage::ClientEvent { .. }
            | ClientMessage::RelayText { .. }
            | ClientMessage::RelayBinary { .. }) => self.route_locally(other).await,
        }
    }

    /// Hand `message` to `to_agent_id`, whose socket the directory places on hub `hub_id`.
    ///
    /// The local session is written directly when that hub is this one; anything else crosses the backplane.
    #[cfg(feature = "nats")]
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

    /// The direct-message path when a NATS backplane decides where the recipient is.
    ///
    /// Every message lands in the shared mailbox first, so it survives the recipient reconnecting to another hub before
    /// it acknowledges, whichever hub it is delivered through.
    #[cfg(feature = "nats")]
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
                self.send_json(&hub::invalid(None, format!("unknown target agent {to_agent_id}")))
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
        let pending = PendingDirectMessage::new(SystemStamps.new_id(), from_agent_id, SystemStamps.now(), message);
        if let Err(error) = nats.queue(&to_agent_id, pending.clone(), self.link.retention()).await {
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
        self.send_json(&hub::status(
            Some(pending.message_id),
            MessageDeliveryStatus::Queued,
            format!("message queued for agent {to_agent_id}"),
        ))
        .await;
    }

    /// Tell the client its request failed because the backplane could not be reached.
    #[cfg(feature = "nats")]
    async fn send_backplane_unavailable(&mut self, error: &BackplaneError) {
        self.send_json(&hub::invalid(None, format!("hub backplane unavailable: {error}")))
            .await;
    }

    /// Deliver `pending`, already in the mailbox, to `to_agent_id` on hub `hub_id`, and report it delivered.
    #[cfg(feature = "nats")]
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
        let (status, detail) = match self
            .deliver_via_backplane(nats, hub_id, to_agent_id, hub::direct_message(pending))
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
        self.send_json(&hub::status(
            Some(message_id),
            status,
            format!("message {detail} agent {to_agent_id}"),
        ))
        .await;
    }

    /// The ack path when the message sits in the shared mailbox, telling its sender wherever that sender is now.
    #[cfg(feature = "nats")]
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
                self.send_json(&hub::invalid(Some(message_id), error.to_string())).await;
                return;
            }
        };
        info!("Agent {recipient_agent_id} acknowledged direct message {message_id} from {sender_agent_id}");
        self.send_json(&hub::status(
            Some(message_id.clone()),
            MessageDeliveryStatus::Acknowledged,
            "message acknowledged",
        ))
        .await;
        self.send_receipt_via_backplane(nats, &recipient_agent_id, &sender_agent_id, message_id)
            .await;
    }

    /// Tell `sender_agent_id`, on whichever hub holds it, that `recipient_agent_id` acknowledged `message_id`.
    #[cfg(feature = "nats")]
    async fn send_receipt_via_backplane(
        &self,
        nats: &NatsBackplane,
        recipient_agent_id: &str,
        sender_agent_id: &str,
        message_id: String,
    ) {
        let receipt = hub::receipt(recipient_agent_id, message_id);
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
                    warn!("Failed to send message to {}: {:?}", self.link.current_agent_id(), err);
                } else {
                    let _sent = HubSpan::start("ws.message.sent");
                }
            }
            Err(error) => {
                error!("Failed to serialize websocket response: {}", error);
            }
        }
    }

    async fn send_text(&mut self, text: String) {
        if let Err(err) = self.session.text(text).await {
            warn!("Failed to forward text to {}: {:?}", self.link.current_agent_id(), err);
        }
    }

    async fn send_binary(&mut self, bytes: Bytes) {
        if let Err(err) = self.session.binary(bytes).await {
            warn!(
                "Failed to forward binary to {}: {:?}",
                self.link.current_agent_id(),
                err
            );
        }
    }

    /// Decode and route one text frame.
    #[expect(
        clippy::cognitive_complexity,
        reason = "the score is info!/warn! expansion; the body branches twice"
    )]
    async fn handle_text(&mut self, text: &str) {
        let _span = HubSpan::start("ws.message.received");
        info!(
            "Received message from client {}: {:?}",
            self.link.current_agent_id(),
            text
        );
        let message = match ClientMessage::from_text_frame(text) {
            Ok(message) => message,
            Err(err) => {
                warn!(
                    "Decode error for et-* frame from {}: {}",
                    self.link.current_agent_id(),
                    err
                );
                return;
            }
        };
        #[cfg(feature = "nats")]
        if let Backplane::Nats(nats) = &self.backplane {
            let nats = nats.clone();
            self.route_via_backplane(&nats, message).await;
            return;
        }
        self.route_locally(message).await;
    }

    /// Route `message` through this hub's own registry, and carry out what that asks.
    async fn route_locally(&mut self, message: ClientMessage) {
        let routing = self.link.route(&self.registry, message, &mut SystemStamps);
        self.apply(routing).await;
    }

    /// Returns `false` when the connection should terminate.
    async fn handle_inbound(&mut self, msg: AggregatedMessage) -> bool {
        telemetry::message_received();
        self.mark_activity();
        match msg {
            AggregatedMessage::Ping(ping) => {
                let _pong: Result<(), actix_ws::Closed> = self.session.pong(&ping).await;
            }
            AggregatedMessage::Pong(_) => {}
            AggregatedMessage::Binary(bytes) => {
                let routing = self.link.route_binary(&self.registry, bytes, &mut SystemStamps);
                self.apply(routing).await;
            }
            AggregatedMessage::Close(reason) => {
                info!(
                    "WebSocket close request from client: {} reason: {:?}",
                    self.link.current_agent_id(),
                    reason
                );
                let _span = HubSpan::start("ws.disconnect");
                let _closed: Result<(), actix_ws::Closed> = self.session.clone().close(reason).await;
                return false;
            }
            AggregatedMessage::Text(text) => self.handle_text(&text).await,
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
            self.link.current_agent_id(),
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
        if !self.registry.blocklist.is_blocked(self.link.client_ip()) {
            return false;
        }
        warn!(
            "Closing WebSocket connection for client {} from blocked IP {}",
            self.link.current_agent_id(),
            self.link.client_ip()
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
        {
            let _connect_span = HubSpan::start("ws.connect");
            info!(
                "WebSocket connection established for client IP {} with agent {}",
                self.link.client_ip(),
                self.link.current_agent_id()
            );
        }
        telemetry::connections_changed(1);

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
                            error!("WebSocket error for client {}: {:?}", self.link.current_agent_id(), e);
                            let _err_span = HubSpan::start("ws.error");
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

        telemetry::connections_changed(-1);
        self.release_identity().await;
    }

    /// Mark this connection's agent disconnected, here and in the shared directory, unless it has been taken over.
    #[cfg_attr(
        feature = "nats",
        expect(
            clippy::cognitive_complexity,
            reason = "the score is info!/warn! expansion; the body branches three times"
        )
    )]
    #[cfg_attr(
        not(feature = "nats"),
        expect(clippy::unused_async, reason = "withdrawing from the backplane is its only await")
    )]
    async fn release_identity(&self) {
        let Some(agent_id) = self.link.agent_id() else {
            info!(
                "WebSocket connection closed before agent assignment for client IP {}",
                self.link.client_ip()
            );
            return;
        };
        // Only this connection's own session: another may have taken the id over since, and is still live.
        #[cfg_attr(
            not(feature = "nats"),
            expect(unused_variables, reason = "only the backplane needs to know it was current")
        )]
        let was_current = self
            .link
            .release(&self.registry, &|session| session.same_channel(self.link.session()));
        #[cfg(feature = "nats")]
        if was_current
            && let Backplane::Nats(nats) = &self.backplane
            && let Err(error) = nats.withdraw(agent_id).await
        {
            warn!("Failed to withdraw agent {agent_id} from the backplane: {error}");
        }
        info!(
            "Agent {} disconnected; last known IP {}",
            agent_id,
            self.link.client_ip()
        );
    }
}

/// `frame` as it crosses the backplane to the other hubs, where a protocol message travels as its JSON text.
#[cfg(feature = "nats")]
#[expect(
    clippy::single_call_fn,
    reason = "the encoding step of share, kept out of its logging"
)]
fn backplane_frame(frame: SessionMessage) -> Result<Frame, serde_json::Error> {
    Ok(match frame {
        SessionMessage::Json(message) => Frame::Text(serde_json::to_string(&message)?),
        SessionMessage::Text(text) => Frame::Text(text),
        SessionMessage::Binary(bytes) => Frame::Binary(bytes),
    })
}
