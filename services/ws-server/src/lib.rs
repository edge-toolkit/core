pub mod config;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use actix::{Actor, ActorContext, Addr, AsyncContext, Handler, Message, StreamHandler};
use actix_files::{Files, NamedFile};
use actix_web::{Error, HttpRequest, HttpResponse, web};
use actix_web_actors::ws;
use chrono::Utc;
use edge_toolkit::ws::{
    AgentConnectionState, AgentSummary, ConnectStatus, MessageDeliveryStatus, MessageScope, WsMessage,
};
use futures_util::StreamExt;
use opentelemetry::{
    global,
    trace::{Span, Tracer},
};
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::config::ModulesConfig;

/// Maximum time the server allows a websocket connection to remain idle before closing it.
/// This should remain comfortably higher than the client's `Alive` message interval.
pub const CONNECTION_TIMEOUT: Duration = Duration::from_secs(15);
/// How often the server checks whether a websocket connection has exceeded `CONNECTION_TIMEOUT`.
/// This is only the check cadence, not the allowed idle duration.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Message)]
#[rtype(result = "()")]
pub struct ServerEnvelope {
    pub message: WsMessage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingDirectMessage {
    pub message_id: String,
    pub from_agent_id: String,
    pub server_received_at: String,
    pub message: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRecord {
    pub state: AgentConnectionState,
    pub last_known_ip: Option<String>,
    #[serde(skip)]
    pub session: Option<Addr<WebSocketActor>>,
    #[serde(default)]
    pub pending_direct_messages: BTreeMap<String, PendingDirectMessage>,
}

#[derive(Clone, Default)]
pub struct AgentRegistry {
    pub agents: Arc<Mutex<BTreeMap<String, AgentRecord>>>,
}

pub enum DirectSendResult {
    Delivered {
        pending: PendingDirectMessage,
        recipient_addr: Addr<WebSocketActor>,
    },
    Queued {
        pending: PendingDirectMessage,
    },
}

pub enum AckResult {
    Acknowledged {
        message_id: String,
        sender_addr: Option<Addr<WebSocketActor>>,
        sender_agent_id: String,
        recipient_agent_id: String,
    },
    Invalid {
        detail: String,
    },
}

impl AgentRegistry {
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let agents = self.agents.lock().expect("agent registry lock poisoned");
        let yaml = serde_yaml::to_string(&*agents).map_err(std::io::Error::other)?;
        std::fs::write(path, yaml)?;
        info!("Agent registry saved to {:?}", path);
        Ok(())
    }

    pub fn load(path: &Path) -> std::io::Result<Self> {
        if !path.exists() {
            warn!("Registry file {:?} does not exist, starting with empty registry", path);
            return Ok(Self::default());
        }
        let yaml = std::fs::read_to_string(path)?;
        let agents: BTreeMap<String, AgentRecord> = serde_yaml::from_str(&yaml).map_err(std::io::Error::other)?;
        info!("Loaded {} agents from registry {:?}", agents.len(), path);
        Ok(Self {
            agents: Arc::new(Mutex::new(agents)),
        })
    }

    pub fn connect_agent(
        &self,
        requested_id: Option<String>,
        client_ip: &str,
        session: Addr<WebSocketActor>,
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

        let assigned_id = Uuid::now_v7().to_string();
        agents.insert(
            assigned_id.clone(),
            AgentRecord {
                state: AgentConnectionState::Connected,
                last_known_ip: Some(client_ip.to_string()),
                session: Some(session),
                pending_direct_messages: BTreeMap::new(),
            },
        );
        (assigned_id, ConnectStatus::Assigned)
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

    pub fn queue_or_deliver_direct(
        &self,
        from_agent_id: &str,
        to_agent_id: &str,
        server_received_at: String,
        message: serde_json::Value,
    ) -> DirectSendResult {
        let mut agents = self.agents.lock().expect("agent registry lock poisoned");
        let recipient = agents
            .get_mut(to_agent_id)
            .expect("queue_or_deliver_direct called for unknown target agent");

        let pending = PendingDirectMessage {
            message_id: Uuid::now_v7().to_string(),
            from_agent_id: from_agent_id.to_string(),
            server_received_at,
            message,
        };
        recipient
            .pending_direct_messages
            .insert(from_agent_id.to_string(), pending);

        if let Some(recipient_addr) = recipient.session.clone() {
            DirectSendResult::Delivered {
                pending: recipient
                    .pending_direct_messages
                    .get(from_agent_id)
                    .expect("pending direct message was just inserted")
                    .clone(),
                recipient_addr,
            }
        } else {
            DirectSendResult::Queued {
                pending: recipient
                    .pending_direct_messages
                    .get(from_agent_id)
                    .expect("pending direct message was just inserted")
                    .clone(),
            }
        }
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

    pub fn acknowledge_message(&self, recipient_agent_id: &str, message_id: &str) -> AckResult {
        let mut agents = self.agents.lock().expect("agent registry lock poisoned");
        let Some(recipient) = agents.get_mut(recipient_agent_id) else {
            return AckResult::Invalid {
                detail: format!("unknown acknowledging agent {}", recipient_agent_id),
            };
        };

        let Some(sender_agent_id) = recipient
            .pending_direct_messages
            .iter()
            .find_map(|(sender_agent_id, pending)| (pending.message_id == message_id).then(|| sender_agent_id.clone()))
        else {
            return AckResult::Invalid {
                detail: "no pending message to acknowledge".to_string(),
            };
        };

        let pending = recipient
            .pending_direct_messages
            .remove(&sender_agent_id)
            .expect("pending direct message disappeared during acknowledgement");
        let sender_addr = agents.get(&sender_agent_id).and_then(|record| record.session.clone());

        AckResult::Acknowledged {
            message_id: pending.message_id,
            sender_addr,
            sender_agent_id,
            recipient_agent_id: recipient_agent_id.to_string(),
        }
    }

    pub fn connected_recipient_addrs(&self, excluding_agent_id: &str) -> Vec<(String, Addr<WebSocketActor>)> {
        let agents = self.agents.lock().expect("agent registry lock poisoned");
        agents
            .iter()
            .filter_map(|(agent_id, record)| {
                if agent_id == excluding_agent_id {
                    return None;
                }
                record.session.clone().map(|addr| (agent_id.clone(), addr))
            })
            .collect()
    }
}

/// WebSocket actor for handling connections.
pub struct WebSocketActor {
    pub agent_id: Option<String>,
    pub last_activity: Instant,
    pub client_ip: String,
    pub registry: AgentRegistry,
}

impl WebSocketActor {
    pub fn new(registry: AgentRegistry, client_ip: String) -> Self {
        info!("New WebSocket actor created for client IP {}", client_ip);
        Self {
            agent_id: None,
            last_activity: Instant::now(),
            client_ip,
            registry,
        }
    }

    pub fn current_agent_id(&self) -> &str {
        self.agent_id.as_deref().unwrap_or("unassigned")
    }

    pub fn assigned_agent_id(&self) -> Option<&str> {
        self.agent_id.as_deref()
    }

    pub fn mark_activity(&mut self) {
        self.last_activity = Instant::now();
    }

    pub fn start_heartbeat(&self, ctx: &mut ws::WebsocketContext<Self>) {
        ctx.run_interval(HEARTBEAT_INTERVAL, |act, ctx| {
            let idle_for = Instant::now().saturating_duration_since(act.last_activity);
            if idle_for > CONNECTION_TIMEOUT {
                warn!(
                    "WebSocket connection timed out for client {} after {:?} of inactivity",
                    act.current_agent_id(),
                    idle_for
                );
                ctx.close(Some(ws::CloseReason {
                    code: ws::CloseCode::Policy,
                    description: Some(format!(
                        "connection timed out after {:?} of inactivity",
                        CONNECTION_TIMEOUT
                    )),
                }));
                ctx.stop();
            }
        });
    }

    fn assign_or_reconnect_agent(
        &mut self,
        requested_id: Option<String>,
        session: Addr<WebSocketActor>,
    ) -> (String, ConnectStatus) {
        let (assigned_id, status) = self.registry.connect_agent(requested_id, &self.client_ip, session);
        self.agent_id = Some(assigned_id.clone());
        (assigned_id, status)
    }

    fn send_json(ctx: &mut ws::WebsocketContext<Self>, response: &WsMessage) {
        match serde_json::to_string(response) {
            Ok(json) => {
                ctx.text(json);
                let tracer = global::tracer("ws-server");
                let mut sent_span = tracer.start("ws.message.sent");
                sent_span.end();
            }
            Err(error) => {
                error!("Failed to serialize websocket response: {}", error);
            }
        }
    }

    fn send_status(
        ctx: &mut ws::WebsocketContext<Self>,
        message_id: Option<String>,
        status: MessageDeliveryStatus,
        detail: impl Into<String>,
    ) {
        Self::send_json(
            ctx,
            &WsMessage::MessageStatus {
                message_id,
                status,
                detail: detail.into(),
            },
        );
    }

    fn send_invalid(ctx: &mut ws::WebsocketContext<Self>, message_id: Option<String>, detail: impl Into<String>) {
        Self::send_json(
            ctx,
            &WsMessage::Invalid {
                message_id,
                detail: detail.into(),
            },
        );
    }

    fn deliver_pending_messages(&self, ctx: &mut ws::WebsocketContext<Self>) {
        let Some(agent_id) = self.assigned_agent_id() else {
            return;
        };
        let pending_messages = self.registry.pending_messages_for(agent_id);
        if pending_messages.is_empty() {
            return;
        }
        for pending in pending_messages {
            info!(
                "Delivering pending message {} to agent {} from {}",
                pending.message_id, agent_id, pending.from_agent_id
            );
            let message = WsMessage::AgentMessage {
                message_id: pending.message_id,
                from_agent_id: pending.from_agent_id,
                scope: MessageScope::Direct,
                server_received_at: pending.server_received_at,
                message: pending.message,
            };
            Self::send_json(ctx, &message);
        }
    }
}

impl Actor for WebSocketActor {
    type Context = ws::WebsocketContext<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.start_heartbeat(ctx);
        info!(
            "WebSocket connection established for client IP {} with agent {}",
            self.client_ip,
            self.current_agent_id()
        );
        let tracer = global::tracer("ws-server");
        let mut span = tracer.start("ws.connect");
        span.end();
    }

    fn stopped(&mut self, _ctx: &mut Self::Context) {
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

impl Handler<ServerEnvelope> for WebSocketActor {
    type Result = ();

    fn handle(&mut self, msg: ServerEnvelope, ctx: &mut Self::Context) -> Self::Result {
        Self::send_json(ctx, &msg.message);
    }
}

impl StreamHandler<Result<ws::Message, ws::ProtocolError>> for WebSocketActor {
    fn handle(&mut self, msg: Result<ws::Message, ws::ProtocolError>, ctx: &mut Self::Context) {
        match msg {
            Ok(ws::Message::Ping(ping)) => {
                self.mark_activity();
                ctx.pong(&ping);
            }
            Ok(ws::Message::Pong(_)) => {
                self.mark_activity();
            }
            Ok(ws::Message::Text(text)) => {
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
                            let (assigned_id, status) = self.assign_or_reconnect_agent(agent_id, ctx.address());
                            info!(
                                "Agent {} status {:?}connected from IP {}",
                                assigned_id, status, self.client_ip
                            );
                            Self::send_json(
                                ctx,
                                &WsMessage::ConnectAck {
                                    agent_id: assigned_id,
                                    status: status.clone(),
                                },
                            );
                            info!(
                                "WebSocket connection ready for client {} with status {:?}",
                                self.current_agent_id(),
                                status
                            );
                            self.deliver_pending_messages(ctx);
                        }
                        WsMessage::Alive { timestamp } => {
                            info!("Alive message from client {} at {}", self.current_agent_id(), timestamp);
                            Self::send_json(
                                ctx,
                                &WsMessage::Response {
                                    message: format!("Alive message received at {}", Utc::now().to_rfc3339()),
                                },
                            );
                        }
                        WsMessage::ListAgents => {
                            let agents = self.registry.list_agents();
                            info!(
                                "Agent {} requested list_agents; returning {} agents",
                                self.current_agent_id(),
                                agents.len()
                            );
                            Self::send_json(ctx, &WsMessage::ListAgentsResponse { agents });
                        }
                        WsMessage::SendAgentMessage { to_agent_id, message } => {
                            let Some(from_agent_id) = self.assigned_agent_id().map(str::to_string) else {
                                Self::send_invalid(ctx, None, "agent must connect before sending messages");
                                span.end();
                                return;
                            };

                            if from_agent_id == to_agent_id {
                                Self::send_invalid(ctx, None, "agent cannot send a direct message to itself");
                                span.end();
                                return;
                            }

                            if !self
                                .registry
                                .list_agents()
                                .iter()
                                .any(|agent| agent.agent_id == to_agent_id)
                            {
                                Self::send_invalid(ctx, None, format!("unknown target agent {}", to_agent_id));
                                span.end();
                                return;
                            }

                            let server_received_at = Utc::now().to_rfc3339();
                            match self.registry.queue_or_deliver_direct(
                                &from_agent_id,
                                &to_agent_id,
                                server_received_at.clone(),
                                message,
                            ) {
                                DirectSendResult::Delivered {
                                    pending,
                                    recipient_addr,
                                } => {
                                    let message_id = pending.message_id.clone();
                                    info!(
                                        "Direct message {} delivered from {} to {}",
                                        message_id, from_agent_id, to_agent_id
                                    );
                                    recipient_addr.do_send(ServerEnvelope {
                                        message: WsMessage::AgentMessage {
                                            message_id: message_id.clone(),
                                            from_agent_id: from_agent_id.clone(),
                                            scope: MessageScope::Direct,
                                            server_received_at: pending.server_received_at,
                                            message: pending.message,
                                        },
                                    });
                                    Self::send_status(
                                        ctx,
                                        Some(message_id),
                                        MessageDeliveryStatus::Delivered,
                                        format!("message delivered to agent {}", to_agent_id),
                                    );
                                }
                                DirectSendResult::Queued { pending } => {
                                    let message_id = pending.message_id;
                                    info!(
                                        "Direct message {} queued from {} to disconnected agent {}",
                                        message_id, from_agent_id, to_agent_id
                                    );
                                    Self::send_status(
                                        ctx,
                                        Some(message_id),
                                        MessageDeliveryStatus::Queued,
                                        format!("message queued for agent {}", to_agent_id),
                                    );
                                }
                            }
                        }
                        WsMessage::BroadcastMessage { message } => {
                            let Some(from_agent_id) = self.assigned_agent_id().map(str::to_string) else {
                                Self::send_invalid(ctx, None, "agent must connect before broadcasting messages");
                                span.end();
                                return;
                            };

                            let recipients = self.registry.connected_recipient_addrs(&from_agent_id);
                            let message_id = Uuid::now_v7().to_string();
                            let server_received_at = Utc::now().to_rfc3339();
                            for (recipient_id, recipient_addr) in &recipients {
                                info!(
                                    "Broadcast message {} from {} to {}",
                                    message_id, from_agent_id, recipient_id
                                );
                                recipient_addr.do_send(ServerEnvelope {
                                    message: WsMessage::AgentMessage {
                                        message_id: message_id.clone(),
                                        from_agent_id: from_agent_id.clone(),
                                        scope: MessageScope::Broadcast,
                                        server_received_at: server_received_at.clone(),
                                        message: message.clone(),
                                    },
                                });
                            }
                            Self::send_status(
                                ctx,
                                Some(message_id),
                                MessageDeliveryStatus::Broadcast,
                                format!("broadcast sent to {} connected agents", recipients.len()),
                            );
                        }
                        WsMessage::MessageAck { message_id } => {
                            let Some(recipient_agent_id) = self.assigned_agent_id().map(str::to_string) else {
                                Self::send_invalid(ctx, None, "agent must connect before acknowledging messages");
                                span.end();
                                return;
                            };

                            match self.registry.acknowledge_message(&recipient_agent_id, &message_id) {
                                AckResult::Acknowledged {
                                    message_id,
                                    sender_addr,
                                    sender_agent_id,
                                    recipient_agent_id,
                                } => {
                                    info!(
                                        "Agent {} acknowledged direct message {} from {}",
                                        recipient_agent_id, message_id, sender_agent_id
                                    );
                                    Self::send_status(
                                        ctx,
                                        Some(message_id.clone()),
                                        MessageDeliveryStatus::Acknowledged,
                                        "message acknowledged",
                                    );
                                    if let Some(sender_addr) = sender_addr {
                                        sender_addr.do_send(ServerEnvelope {
                                            message: WsMessage::MessageStatus {
                                                message_id: Some(message_id),
                                                status: MessageDeliveryStatus::Acknowledged,
                                                detail: format!("agent {} acknowledged receipt", recipient_agent_id),
                                            },
                                        });
                                    }
                                }
                                AckResult::Invalid { detail } => {
                                    warn!("Invalid ack from {} for {}: {}", recipient_agent_id, message_id, detail);
                                    Self::send_invalid(ctx, Some(message_id), detail);
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
                                    .and_then(|value| value.as_f64())
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
                        WsMessage::StoreFile { filename } => {
                            let Some(agent_id) = self.assigned_agent_id() else {
                                Self::send_invalid(ctx, None, "agent must connect before storing files");
                                span.end();
                                return;
                            };
                            let url = format!("/storage/{}/{}", agent_id, filename);
                            info!("Agent {} requested storage URL for {}: {}", agent_id, filename, url);
                            Self::send_json(
                                ctx,
                                &WsMessage::Response {
                                    message: format!("PUT to {}", url),
                                },
                            );
                        }
                        WsMessage::FetchFile { agent_id, filename } => {
                            let url = format!("/storage/{}/{}", agent_id, filename);
                            info!(
                                "Agent {} requested fetch URL for {}/{}",
                                self.current_agent_id(),
                                agent_id,
                                filename
                            );
                            Self::send_json(
                                ctx,
                                &WsMessage::Response {
                                    message: format!("GET from {}", url),
                                },
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
                } else {
                    warn!(
                        "Received unrecognized message from client {}: {}",
                        self.current_agent_id(),
                        text
                    );
                }
                span.end();
            }
            Ok(ws::Message::Close(reason)) => {
                self.mark_activity();
                info!(
                    "WebSocket close request from client: {} reason: {:?}",
                    self.current_agent_id(),
                    reason
                );
                let tracer = global::tracer("ws-server");
                let mut span = tracer.start("ws.disconnect");
                span.end();
                ctx.close(reason);
                ctx.stop();
            }
            Ok(ws::Message::Binary(_)) | Ok(ws::Message::Continuation(_)) | Ok(ws::Message::Nop) => {
                self.mark_activity();
            }
            Err(e) => {
                error!("WebSocket error for client {}: {:?}", self.current_agent_id(), e);
                let tracer = global::tracer("ws-server");
                let mut span = tracer.start("ws.error");
                span.end();
            }
        }
    }
}

/// WebSocket endpoint handler.
pub async fn ws_handler(
    req: HttpRequest,
    stream: web::Payload,
    registry: web::Data<AgentRegistry>,
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

    let actor = WebSocketActor::new(registry.get_ref().clone(), client_ip);
    let result = ws::start(actor, &req, stream);

    span.end();
    result
}

pub fn browser_static_dir() -> PathBuf {
    Path::new(".").join("static")
}

pub async fn browser_index() -> Result<NamedFile, Error> {
    let path = browser_static_dir().join("index.html");
    info!("Serving browser interface page: {:?}", path);

    NamedFile::open(path).map_err(|e| {
        error!("Failed to open browser interface page: {}", e);
        actix_web::error::ErrorNotFound(e)
    })
}

pub async fn no_content() -> HttpResponse {
    HttpResponse::NoContent().finish()
}

/// Static file handler — serves a named binary file from the ws-server static directory.
/// Example: GET /files/firmware.bin  → services/ws-server/static/firmware.bin
pub async fn file_handler(req: HttpRequest) -> Result<NamedFile, Error> {
    // Extract the filename segment from the URL path.
    let filename: PathBuf = req
        .match_info()
        .query("filename")
        .parse()
        .map_err(|_| actix_web::error::ErrorBadRequest("invalid filename"))?;

    // Prevent directory traversal: reject any path containing a separator.
    if filename.components().count() != 1 {
        return Err(actix_web::error::ErrorBadRequest("invalid filename"));
    }

    let path = browser_static_dir().join(&filename);

    info!("Serving static file: {:?}", path);

    let file = NamedFile::open(&path).map_err(|e| {
        error!("Failed to open static file {:?}: {}", path, e);
        actix_web::error::ErrorNotFound(e)
    })?;

    // Treat every file as an opaque binary stream so browsers don't
    // try to render or sniff the content type.
    Ok(file
        .use_etag(true)
        .use_last_modified(true)
        .set_content_type(actix_web::mime::APPLICATION_OCTET_STREAM))
}

pub async fn health() -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({
        "status": "healthy",
        "service": "ws-server"
    }))
}

/// Scan all configured module paths and return a sorted list of (name, pkg_dir) pairs.
/// Each entry is a module whose `<dir>/pkg/` subdirectory exists.
pub fn list_modules(config: &ModulesConfig) -> Vec<(String, PathBuf)> {
    let mut modules: Vec<(String, PathBuf)> = Vec::new();
    for path in &config.paths {
        let pkg_dir = path.join("pkg");
        if pkg_dir.is_dir() {
            // This path is itself a single module (e.g. ws-wasm-agent).
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                modules.push((name.to_string(), pkg_dir));
            }
        } else if let Ok(entries) = std::fs::read_dir(path) {
            // This path is a directory of modules (e.g. ws-modules).
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type()
                    && file_type.is_dir()
                    && let Some(name) = entry.file_name().to_str()
                {
                    let pkg_dir = entry.path().join("pkg");
                    if pkg_dir.is_dir() {
                        modules.push((name.to_string(), pkg_dir));
                    }
                }
            }
        }
    }
    modules.sort_by(|a, b| a.0.cmp(&b.0));
    modules
}

async fn api_list_modules(modules_config: web::Data<ModulesConfig>) -> HttpResponse {
    let names: Vec<String> = list_modules(&modules_config)
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| name != "ws-wasm-agent")
        .collect();
    HttpResponse::Ok().json(names)
}

pub fn workspace_root() -> PathBuf {
    edge_toolkit::config::get_project_root()
}

pub fn wasm_pkg_dir() -> PathBuf {
    workspace_root().join("services/ws-wasm-agent/pkg")
}

pub fn wasm_modules_dir() -> PathBuf {
    workspace_root().join("services/ws-modules")
}

pub async fn agent_put_file(
    req: HttpRequest,
    mut payload: web::Payload,
    registry: web::Data<AgentRegistry>,
) -> Result<HttpResponse, Error> {
    let agent_id: String = req.match_info().query("agent_id").parse().unwrap();
    let filename: PathBuf = req
        .match_info()
        .query("filename")
        .parse()
        .map_err(|_| actix_web::error::ErrorBadRequest("invalid filename"))?;

    {
        let agents = registry.agents.lock().expect("lock poisoned");
        if !agents.contains_key(&agent_id) {
            return Err(actix_web::error::ErrorNotFound("agent not found"));
        }
    }

    if filename.components().count() != 1 {
        return Err(actix_web::error::ErrorBadRequest("invalid filename"));
    }

    let storage_dir = Path::new(".").join("storage");
    let agent_dir = storage_dir.join(&agent_id);
    std::fs::create_dir_all(&agent_dir)?;

    let path = agent_dir.join(&filename);
    info!("Agent {} storing file: {:?}", agent_id, path);

    let mut file = tokio::fs::File::create(path).await?;
    while let Some(chunk) = payload.next().await {
        let chunk = chunk?;
        tokio::io::copy(&mut &chunk[..], &mut file).await?;
    }

    Ok(HttpResponse::Ok().finish())
}

pub fn configure_app(
    cfg: &mut web::ServiceConfig,
    agent_registry: web::Data<AgentRegistry>,
    storage_dir: PathBuf,
    modules_config: ModulesConfig,
) {
    let modules = list_modules(&modules_config);
    cfg.app_data(agent_registry)
        .app_data(web::Data::new(modules_config))
        .route("/", web::get().to(browser_index))
        .route("/index.html", web::get().to(browser_index))
        .route("/favicon.ico", web::get().to(no_content))
        .route("/health", web::get().to(health))
        .route("/api/modules", web::get().to(api_list_modules))
        .route("/ws", web::get().to(ws_handler))
        .route("/files/{filename}", web::get().to(file_handler))
        .route("/storage/{agent_id}/{filename}", web::put().to(agent_put_file))
        .service(
            Files::new("/storage", storage_dir)
                .show_files_listing()
                .use_etag(true)
                .use_last_modified(true),
        )
        .service(Files::new("/static", browser_static_dir()).prefer_utf8(true));
    for (name, pkg_dir) in modules {
        cfg.service(Files::new(&format!("/modules/{name}"), pkg_dir).prefer_utf8(true));
    }
}
