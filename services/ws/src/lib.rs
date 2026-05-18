use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use actix_web::{Error, HttpRequest, HttpResponse, web};
use actix_ws::{AggregatedMessage, AggregatedMessageStream, CloseCode, CloseReason, Session};
use chrono::Utc;
use edge_toolkit::ws::{ConnectStatus, MessageDeliveryStatus, MessageScope, WsMessage};
use edge_toolkit::ws_server::{AgentRecord, AgentRegistry, PendingDirectMessage};
use futures_util::StreamExt as _;
use opentelemetry::{
    global,
    trace::{Span, Tracer},
};
use serde::Deserialize;
use serde_default::DefaultFromSerde;
use serde_inline_default::serde_inline_default;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tracing::{error, info, warn};
use uuid::Uuid;

pub const CONNECTION_TIMEOUT: Duration = Duration::from_secs(15);
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);

/// Default max WebSocket frame size (64 MiB).
///
/// Matches the split-learning demo's `ws_max_size`; activation / gradient
/// tensors fanned out via default broadcast easily blow past actix-ws's
/// 64 KiB default. Override via the `WS_MAX_FRAME_SIZE` env var
/// (`serde-env` translates `[ws] max_frame_size` to `WS_MAX_FRAME_SIZE`).
pub const DEFAULT_MAX_FRAME_SIZE: usize = 64 * 1024 * 1024;

/// Runtime knobs for the WebSocket hub. Populated by `serde-env` in
/// `et-ws-server::main`, then handed to `configure`.
#[serde_inline_default]
#[derive(Clone, Debug, DefaultFromSerde, Deserialize)]
pub struct WsConfig {
    /// Largest single WebSocket frame the hub will accept (bytes). Frames
    /// above this are dropped by actix-ws before they reach the handler, so
    /// callers shipping big tensors / blobs need to raise this above their
    /// payload size.
    #[serde_inline_default(DEFAULT_MAX_FRAME_SIZE)]
    pub max_frame_size: usize,
}

/// One frame queued on an agent's outbound channel.
///
/// Carries either an et-typed `WsMessage` (the protocol envelope) or a raw
/// WebSocket frame that the server is relaying unchanged. The latter is how
/// default broadcasting works: when a peer sends a frame the server doesn't
/// recognise as a `WsMessage`, the server fans it out to other connected
/// agents as the original `Text` / `Binary` frame.
#[derive(Debug, Clone)]
pub enum OutboundFrame {
    /// An et-typed protocol message; serialised to JSON and sent as a text frame.
    Message(WsMessage),
    /// Raw text frame forwarded as-is (default broadcast of an unrecognised text payload).
    Text(String),
    /// Raw binary frame forwarded as-is (default broadcast of a binary payload).
    Binary(Vec<u8>),
}

impl From<WsMessage> for OutboundFrame {
    fn from(message: WsMessage) -> Self {
        Self::Message(message)
    }
}

pub type AgentSession = UnboundedSender<OutboundFrame>;
pub type WsAgentRegistry = AgentRegistry<AgentSession>;

/// Load a registry from disk. Sessions are not persisted, so they are initialised to `None`.
pub fn load_registry(path: &std::path::Path) -> Result<WsAgentRegistry, std::io::Error> {
    use edge_toolkit::ws::AgentConnectionState;
    if !path.exists() {
        warn!("Registry file {:?} does not exist, starting with empty registry", path);
        return Ok(WsAgentRegistry::default());
    }
    let yaml = std::fs::read_to_string(path)?;
    // Deserialize using a session-less record type, then convert.
    #[derive(serde::Deserialize)]
    struct BareRecord {
        state: AgentConnectionState,
        last_known_ip: Option<String>,
        #[serde(default)]
        pending_direct_messages: BTreeMap<String, PendingDirectMessage>,
    }
    let bare: BTreeMap<String, BareRecord> = serde_yaml::from_str(&yaml).map_err(std::io::Error::other)?;
    let agents = bare
        .into_iter()
        .map(|(id, r)| {
            (
                id,
                AgentRecord {
                    state: r.state,
                    last_known_ip: r.last_known_ip,
                    session: None,
                    pending_direct_messages: r.pending_direct_messages,
                },
            )
        })
        .collect();
    info!("Loaded registry from {:?}", path);
    Ok(WsAgentRegistry {
        agents: Arc::new(Mutex::new(agents)),
    })
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
            let _ = recipient.send(OutboundFrame::Message(WsMessage::AgentMessage {
                message_id: message_id.clone(),
                from_agent_id,
                scope: MessageScope::Direct,
                server_received_at: pending.server_received_at,
                message: pending.message,
            }));
            self.send_status(
                Some(message_id),
                MessageDeliveryStatus::Delivered,
                format!("message delivered to agent {}", to_agent_id),
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
                format!("message queued for agent {}", to_agent_id),
            )
            .await;
        }
        span.end();
    }

    /// Forward a raw frame to every other connected agent.
    ///
    /// This is the default broadcast path: any text frame the server can't
    /// parse as an et-typed `WsMessage`, and every binary frame, ends up
    /// here. Frames are relayed unchanged so foreign schemas (the
    /// split-learning demo's base64-encoded tensor payloads, etc.) pass
    /// through without re-wrapping.
    fn broadcast_raw_frame(&self, frame: OutboundFrame) -> usize {
        let Some(from_agent_id) = self.assigned_agent_id() else {
            return 0;
        };
        let recipients = self.registry.connected_sessions(from_agent_id);
        for (_, recipient) in &recipients {
            let _ = recipient.send(frame.clone());
        }
        recipients.len()
    }

    /// Returns `false` when the connection should terminate.
    async fn handle_inbound(&mut self, msg: AggregatedMessage) -> bool {
        match msg {
            AggregatedMessage::Ping(ping) => {
                self.mark_activity();
                let _ = self.session.pong(&ping).await;
            }
            AggregatedMessage::Pong(_) => {
                self.mark_activity();
            }
            AggregatedMessage::Binary(bytes) => {
                self.mark_activity();
                let tracer = global::tracer("ws-server");
                let mut span = tracer.start("ws.message.received");
                let Some(from_agent_id) = self.assigned_agent_id().map(str::to_string) else {
                    self.send_invalid(None, "agent must connect before sending binary frames")
                        .await;
                    span.end();
                    return true;
                };
                let len = bytes.len();
                let count = self.broadcast_raw_frame(OutboundFrame::Binary(bytes.to_vec()));
                info!(
                    "Default-broadcast {}-byte binary frame from {} to {} agent(s)",
                    len, from_agent_id, count
                );
                span.end();
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
                let _ = self.session.clone().close(reason).await;
                return false;
            }
            AggregatedMessage::Text(text) => {
                self.mark_activity();
                let tracer = global::tracer("ws-server");
                let mut span = tracer.start("ws.message.received");
                info!("Received message from client {}: {:?}", self.current_agent_id(), text);

                let parsed = serde_json::from_str::<WsMessage>(&text);
                if let Ok(msg) = parsed {
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

                            if !self.registry.list_agents().iter().any(|a| a.agent_id == to_agent_id) {
                                self.send_invalid(None, format!("unknown target agent {}", to_agent_id))
                                    .await;
                                span.end();
                                return true;
                            }

                            self.handle_send_direct(&mut span, from_agent_id, to_agent_id, message)
                                .await;
                            return true;
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
                                        let _ = sender.send(OutboundFrame::Message(WsMessage::MessageStatus {
                                            message_id: Some(message_id),
                                            status: MessageDeliveryStatus::Acknowledged,
                                            detail: format!("agent {} acknowledged receipt", recipient_agent_id),
                                        }));
                                    }
                                }
                                Err(detail) => {
                                    warn!("Invalid ack from {} for {}: {}", recipient_agent_id, message_id, detail);
                                    self.send_invalid(Some(message_id), detail).await;
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
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("unknown");
                                let confidence = details.get("confidence").and_then(|v| v.as_f64()).unwrap_or_default();
                                let processed_at = details
                                    .get("processed_at")
                                    .and_then(|v| v.as_str())
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
                        WsMessage::StoreFile { filename } => {
                            let Some(agent_id) = self.assigned_agent_id().map(str::to_string) else {
                                self.send_invalid(None, "agent must connect before storing files").await;
                                span.end();
                                return true;
                            };
                            let url = format!("/storage/{}/{}", agent_id, filename);
                            info!("Agent {} requested storage URL for {}: {}", agent_id, filename, url);
                            self.send_json(&WsMessage::Response {
                                message: format!("PUT to {}", url),
                            })
                            .await;
                        }
                        WsMessage::FetchFile { agent_id, filename } => {
                            let url = format!("/storage/{}/{}", agent_id, filename);
                            info!(
                                "Agent {} requested fetch URL for {}/{}",
                                self.current_agent_id(),
                                agent_id,
                                filename
                            );
                            self.send_json(&WsMessage::Response {
                                message: format!("GET from {}", url),
                            })
                            .await;
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
                } else {
                    // Default broadcast: forward unrecognised text frames as-is
                    // so foreign protocols (e.g. split-learning) flow through.
                    let Some(from_agent_id) = self.assigned_agent_id().map(str::to_string) else {
                        self.send_invalid(None, "agent must connect before broadcasting messages")
                            .await;
                        span.end();
                        return true;
                    };
                    let count = self.broadcast_raw_frame(OutboundFrame::Text(text.to_string()));
                    info!(
                        "Default-broadcast unrecognised text frame from {} to {} agent(s)",
                        from_agent_id, count
                    );
                }
                span.end();
            }
        }
        true
    }

    async fn send_frame(&mut self, frame: OutboundFrame) {
        match frame {
            OutboundFrame::Message(msg) => self.send_json(&msg).await,
            OutboundFrame::Text(text) => {
                if let Err(err) = self.session.text(text).await {
                    warn!("Failed to forward text frame to {}: {:?}", self.current_agent_id(), err);
                }
            }
            OutboundFrame::Binary(bytes) => {
                if let Err(err) = self.session.binary(bytes).await {
                    warn!(
                        "Failed to forward binary frame to {}: {:?}",
                        self.current_agent_id(),
                        err
                    );
                }
            }
        }
    }

    async fn run(mut self, mut stream: AggregatedMessageStream, mut outbound: UnboundedReceiver<OutboundFrame>) {
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
                Some(frame) = outbound.recv() => {
                    self.send_frame(frame).await;
                }
                _ = heartbeat.tick() => {
                    let idle_for = Instant::now().saturating_duration_since(self.last_activity);
                    if idle_for > CONNECTION_TIMEOUT {
                        warn!(
                            "WebSocket connection timed out for client {} after {:?} of inactivity",
                            self.current_agent_id(),
                            idle_for
                        );
                        let _ = self.session.clone().close(Some(CloseReason {
                            code: CloseCode::Policy,
                            description: Some(format!(
                                "connection timed out after {:?} of inactivity",
                                CONNECTION_TIMEOUT
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

pub async fn ws_handler(
    req: HttpRequest,
    body: web::Payload,
    registry: web::Data<WsAgentRegistry>,
    config: web::Data<WsConfig>,
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
    let stream = msg_stream
        .max_frame_size(config.max_frame_size)
        .aggregate_continuations();

    let (tx, rx) = mpsc::unbounded_channel::<OutboundFrame>();
    let conn = Connection::new(registry.get_ref().clone(), client_ip, session, tx);

    actix_web::rt::spawn(async move {
        conn.run(stream, rx).await;
    });

    span.end();
    Ok(response)
}

pub fn configure(cfg: &mut web::ServiceConfig, config: &WsConfig) {
    cfg.app_data(web::Data::new(config.clone()))
        .route("/ws", web::get().to(ws_handler));
}
