use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use actix_web::{Error, HttpRequest, HttpResponse, web};
use actix_ws::{AggregatedMessage, AggregatedMessageStream, CloseCode, CloseReason, Session};
use bytes::Bytes;
use chrono::Utc;
use edge_toolkit::ws::{ConnectStatus, MessageDeliveryStatus, MessageScope, WsMessage};
use edge_toolkit::ws_server::{AgentRecord, AgentRegistry, PendingDirectMessage, RegistryError};
use futures_util::StreamExt as _;
use opentelemetry::{
    global,
    trace::{Span, Tracer as _},
};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tracing::{error, info, warn};
use uuid::Uuid;

pub const CONNECTION_TIMEOUT: Duration = Duration::from_secs(15);
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);

/// Outbound envelope written to an agent's websocket session.
///
/// `Json` is the normal path for protocol messages. `Text` and `Binary` carry
/// payloads the server forwards verbatim — used by the hub-style fallback
/// that broadcasts unrecognised frames to every other connected agent.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum SessionMessage {
    Json(WsMessage),
    Text(String),
    Binary(Bytes),
}

impl From<WsMessage> for SessionMessage {
    fn from(value: WsMessage) -> Self {
        Self::Json(value)
    }
}

pub type AgentSession = UnboundedSender<SessionMessage>;
pub type WsAgentRegistry = AgentRegistry<AgentSession>;

// Deserialize using a session-less record type, then convert.
#[derive(serde::Deserialize)]
struct BareRecord {
    state: edge_toolkit::ws::AgentConnectionState,
    last_known_ip: Option<String>,
    #[serde(default)]
    pending_direct_messages: BTreeMap<String, PendingDirectMessage>,
}

/// Load a registry from disk. Sessions are not persisted, so they are initialised to `None`.
pub fn load_registry(path: &std::path::Path) -> Result<WsAgentRegistry, RegistryError> {
    if !path.exists() {
        warn!(
            "Registry file {} does not exist, starting with empty registry",
            path.display()
        );
        return Ok(WsAgentRegistry::default());
    }
    let yaml = std::fs::read_to_string(path)?;
    let bare: BTreeMap<String, BareRecord> = serde_yaml::from_str(&yaml)?;
    let agents = bare
        .into_iter()
        .map(|(id, record)| {
            (
                id,
                AgentRecord::new(record.state, record.last_known_ip, None)
                    .with_pending_direct_messages(record.pending_direct_messages),
            )
        })
        .collect();
    info!("Loaded registry from {}", path.display());
    Ok(WsAgentRegistry::from_agents(agents))
}

struct Connection {
    agent_id: Option<String>,
    last_activity: Instant,
    client_ip: String,
    registry: WsAgentRegistry,
    session: Session,
    outbox: AgentSession,
}

impl Connection {
    #[expect(clippy::single_call_fn, reason = "inherent constructor; used once by ws_handler")]
    fn new(registry: WsAgentRegistry, client_ip: String, session: Session, outbox: AgentSession) -> Self {
        info!("New WebSocket connection for client IP {}", client_ip);
        Self {
            agent_id: None,
            last_activity: Instant::now(),
            client_ip,
            registry,
            session,
            outbox,
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
                .connect_agent(requested_id, new_id, &self.client_ip, self.outbox.clone());
        self.agent_id = Some(assigned_id.clone());
        (assigned_id, status)
    }

    async fn send_json(&mut self, response: &WsMessage) {
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
        self.send_json(&WsMessage::MessageStatus {
            message_id,
            status,
            detail: detail.into(),
        })
        .await;
    }

    async fn send_invalid(&mut self, message_id: Option<String>, detail: impl Into<String>) {
        self.send_json(&WsMessage::Invalid {
            message_id,
            detail: detail.into(),
        })
        .await;
    }

    async fn deliver_pending_messages(&mut self) {
        let Some(agent_id) = self.assigned_agent_id().map(str::to_string) else {
            return;
        };
        for pending in self.registry.pending_messages_for(&agent_id) {
            info!(
                "Delivering pending message {} to agent {} from {}",
                pending.message_id, agent_id, pending.from_agent_id
            );
            self.send_json(&WsMessage::AgentMessage {
                message_id: pending.message_id,
                from_agent_id: pending.from_agent_id,
                scope: MessageScope::Direct,
                server_received_at: pending.server_received_at,
                message: pending.message,
            })
            .await;
        }
    }

    async fn handle_send_direct(
        &mut self,
        span: &mut impl Span,
        from_agent_id: String,
        to_agent_id: String,
        message: serde_json::Value,
    ) {
        let server_received_at = Utc::now().to_rfc3339();
        let (pending, recipient_session) = self.registry.queue_direct(
            Uuid::now_v7().to_string(),
            &from_agent_id,
            &to_agent_id,
            server_received_at,
            message,
        );
        let message_id = pending.message_id.clone();

        if let Some(recipient) = recipient_session {
            info!(
                "Direct message {} delivered from {} to {}",
                message_id, from_agent_id, to_agent_id
            );
            drop(recipient.send(SessionMessage::Json(WsMessage::AgentMessage {
                message_id: message_id.clone(),
                from_agent_id,
                scope: MessageScope::Direct,
                server_received_at: pending.server_received_at,
                message: pending.message,
            })));
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

    /// Hub-style fallback: forward raw text to every connected agent except
    /// the sender. Used when a frame doesn't parse as a known `WsMessage`.
    fn broadcast_raw_text(&self, from_agent_id: &str, text: &str) {
        let recipients = self.registry.connected_sessions(from_agent_id);
        info!(
            "Broadcasting unrecognised text message from {} to {} agent(s)",
            from_agent_id,
            recipients.len()
        );
        for (_, recipient) in recipients {
            drop(recipient.send(SessionMessage::Text(text.to_string())));
        }
    }

    /// Hub-style fallback for binary frames — same shape as the text path.
    fn broadcast_raw_binary(&self, from_agent_id: &str, bytes: &Bytes) {
        let recipients = self.registry.connected_sessions(from_agent_id);
        info!(
            "Broadcasting unrecognised binary message ({} bytes) from {} to {} agent(s)",
            bytes.len(),
            from_agent_id,
            recipients.len()
        );
        for (_, recipient) in recipients {
            drop(recipient.send(SessionMessage::Binary(bytes.clone())));
        }
    }

    /// Returns `false` when the connection should terminate.
    #[expect(
        clippy::cognitive_complexity,
        clippy::too_many_lines,
        reason = "single dispatcher for inbound WsMessage variants; splitting scatters handlers into trivial helpers"
    )]
    async fn handle_inbound(&mut self, msg: AggregatedMessage) -> bool {
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
                if let Some(from_agent_id) = self.assigned_agent_id().map(str::to_string) {
                    self.broadcast_raw_binary(&from_agent_id, &bytes);
                } else {
                    warn!(
                        "Dropping binary frame from unassigned client {}: agent must connect first",
                        self.client_ip
                    );
                }
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

                if let Ok(msg) = serde_json::from_str::<WsMessage>(&text) {
                    match msg {
                        WsMessage::Connect { agent_id } => {
                            let requested_id = agent_id.clone();
                            info!(
                                "Connect message: requested_agent_id={:?} client_ip={}",
                                requested_id, self.client_ip
                            );
                            let (assigned_id, status) = self.assign_or_reconnect_agent(agent_id);
                            info!(
                                "Agent {} status {:?}connected from IP {}",
                                assigned_id, status, self.client_ip
                            );
                            self.send_json(&WsMessage::ConnectAck {
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
                        WsMessage::Alive { timestamp } => {
                            info!("Alive message from client {} at {}", self.current_agent_id(), timestamp);
                            self.send_json(&WsMessage::Response {
                                message: format!("Alive message received at {}", Utc::now().to_rfc3339()),
                            })
                            .await;
                        }
                        WsMessage::ListAgents => {
                            let agents = self.registry.list_agents();
                            info!(
                                "Agent {} requested list_agents; returning {} agents",
                                self.current_agent_id(),
                                agents.len()
                            );
                            self.send_json(&WsMessage::ListAgentsResponse { agents }).await;
                        }
                        WsMessage::SendAgentMessage { to_agent_id, message } => {
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

                            if !self
                                .registry
                                .list_agents()
                                .iter()
                                .any(|agent| agent.agent_id == to_agent_id)
                            {
                                self.send_invalid(None, format!("unknown target agent {to_agent_id}"))
                                    .await;
                                span.end();
                                return true;
                            }

                            self.handle_send_direct(&mut span, from_agent_id, to_agent_id, message)
                                .await;
                            return true;
                        }
                        WsMessage::BroadcastMessage { message } => {
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
                            for (_, recipient) in &recipients {
                                drop(recipient.send(SessionMessage::Json(WsMessage::AgentMessage {
                                    message_id: message_id.clone(),
                                    from_agent_id: from_agent_id.clone(),
                                    scope: MessageScope::Broadcast,
                                    server_received_at: server_received_at.clone(),
                                    message: message.clone(),
                                })));
                            }
                            self.send_status(
                                Some(message_id),
                                MessageDeliveryStatus::Broadcast,
                                format!("broadcast sent to {} connected agents", recipients.len()),
                            )
                            .await;
                        }
                        WsMessage::MessageAck { message_id } => {
                            let Some(recipient_agent_id) = self.assigned_agent_id().map(str::to_string) else {
                                self.send_invalid(None, "agent must connect before acknowledging messages")
                                    .await;
                                span.end();
                                return true;
                            };

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
                                        drop(sender.send(SessionMessage::Json(WsMessage::MessageStatus {
                                            message_id: Some(message_id),
                                            status: MessageDeliveryStatus::Acknowledged,
                                            detail: format!("agent {recipient_agent_id} acknowledged receipt"),
                                        })));
                                    }
                                }
                                Err(error) => {
                                    warn!("Invalid ack from {} for {}: {}", recipient_agent_id, message_id, error);
                                    self.send_invalid(Some(message_id), error.to_string()).await;
                                }
                            }
                        }
                        WsMessage::ClientEvent {
                            capability,
                            action,
                            details,
                        } => {
                            if capability == "video_cv" && action == "inference" {
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
                                    self.current_agent_id(),
                                    detected_class,
                                    confidence,
                                    processed_at
                                );
                            }
                            info!(
                                "Client event from {}: capability={} action={} details={}",
                                self.current_agent_id(),
                                capability,
                                action,
                                details
                            );
                        }
                        WsMessage::ConnectAck { .. }
                        | WsMessage::ListAgentsResponse { .. }
                        | WsMessage::AgentMessage { .. }
                        | WsMessage::MessageStatus { .. }
                        | WsMessage::Invalid { .. }
                        | WsMessage::Response { .. } => {
                            warn!(
                                "Unexpected server-originated message from client {}",
                                self.current_agent_id()
                            );
                        }
                    }
                } else if let Some(from_agent_id) = self.assigned_agent_id().map(str::to_string) {
                    self.broadcast_raw_text(&from_agent_id, &text);
                } else {
                    warn!(
                        "Dropping unrecognised text from unassigned client {}: agent must connect first",
                        self.client_ip
                    );
                }
                span.end();
            }
        }
        true
    }

    #[expect(
        clippy::cognitive_complexity,
        clippy::future_not_send,
        clippy::integer_division_remainder_used,
        reason = "actix-ws AggregatedMessageStream is Rc-backed and !Send; tokio::select! macro uses % internally"
    )]
    async fn run(mut self, mut stream: AggregatedMessageStream, mut outbound: UnboundedReceiver<SessionMessage>) {
        let tracer = global::tracer("ws-server");
        let mut connect_span = tracer.start("ws.connect");
        info!(
            "WebSocket connection established for client IP {} with agent {}",
            self.client_ip,
            self.current_agent_id()
        );
        connect_span.end();

        let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                msg = stream.next() => {
                    match msg {
                        Some(Ok(msg)) => {
                            if !self.handle_inbound(msg).await {
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
                    let idle_for = Instant::now().saturating_duration_since(self.last_activity);
                    if idle_for > CONNECTION_TIMEOUT {
                        warn!(
                            "WebSocket connection timed out for client {} after {:?} of inactivity",
                            self.current_agent_id(),
                            idle_for
                        );
                        let _closed: Result<(), actix_ws::Closed> = self.session.clone().close(Some(CloseReason {
                            code: CloseCode::Policy,
                            description: Some(format!(
                                "connection timed out after {CONNECTION_TIMEOUT:?} of inactivity"
                            )),
                        })).await;
                        break;
                    }
                }
            }
        }

        if let Some(agent_id) = self.agent_id.as_deref() {
            self.registry.mark_disconnected(agent_id);
            info!("Agent {} disconnected; last known IP {}", agent_id, self.client_ip);
        } else {
            info!(
                "WebSocket connection closed before agent assignment for client IP {}",
                self.client_ip
            );
        }
    }
}

#[expect(
    clippy::future_not_send,
    reason = "actix-web HttpRequest and Payload are Rc-backed and !Send; handler runs on actix's single thread"
)]
pub async fn ws_handler(
    req: HttpRequest,
    body: web::Payload,
    registry: web::Data<WsAgentRegistry>,
) -> Result<HttpResponse, Error> {
    let tracer = global::tracer("ws-server");
    let mut span = tracer.start("ws.connect");

    let client_ip = req
        .peer_addr()
        .map(|addr| addr.ip().to_string())
        .or_else(|| {
            req.connection_info()
                .realip_remote_addr()
                .and_then(|addr| addr.split(':').next().map(str::to_string))
        })
        .unwrap_or_else(|| "unknown".to_string());

    let (response, session, msg_stream) = actix_ws::handle(&req, body)?;
    let stream = msg_stream.max_frame_size(64 * 1024).aggregate_continuations();

    let (tx, rx) = mpsc::unbounded_channel::<SessionMessage>();
    let conn = Connection::new(registry.get_ref().clone(), client_ip, session, tx);

    let _join = actix_web::rt::spawn(async move {
        conn.run(stream, rx).await;
    });

    span.end();
    Ok(response)
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    let _routed = cfg.route("/ws", web::get().to(ws_handler));
}
