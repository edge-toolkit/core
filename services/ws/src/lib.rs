use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use actix::{Actor, ActorContext, Addr, AsyncContext, Handler, Message, StreamHandler};
use actix_web::{Error, HttpRequest, HttpResponse, web};
use actix_web_actors::ws;
use chrono::Utc;
use edge_toolkit::ws::{ConnectStatus, MessageDeliveryStatus, MessageScope, WsMessage};
use edge_toolkit::ws_server::{AgentRecord, AgentRegistry, PendingDirectMessage};
use opentelemetry::{
    global,
    trace::{Span, Tracer},
};
use tracing::{error, info, warn};
use uuid::Uuid;

pub const CONNECTION_TIMEOUT: Duration = Duration::from_secs(15);
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);

pub type WsAgentRegistry = AgentRegistry<Addr<WebSocketActor>>;

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

#[derive(Message)]
#[rtype(result = "()")]
pub struct ServerEnvelope {
    pub message: WsMessage,
}

pub struct WebSocketActor {
    pub agent_id: Option<String>,
    pub last_activity: Instant,
    pub client_ip: String,
    pub registry: WsAgentRegistry,
}

impl WebSocketActor {
    pub fn new(registry: WsAgentRegistry, client_ip: String) -> Self {
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
        let new_id = Uuid::now_v7().to_string();
        let (assigned_id, status) = self
            .registry
            .connect_agent(requested_id, new_id, &self.client_ip, session);
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
        for pending in self.registry.pending_messages_for(agent_id) {
            info!(
                "Delivering pending message {} to agent {} from {}",
                pending.message_id, agent_id, pending.from_agent_id
            );
            Self::send_json(
                ctx,
                &WsMessage::AgentMessage {
                    message_id: pending.message_id,
                    from_agent_id: pending.from_agent_id,
                    scope: MessageScope::Direct,
                    server_received_at: pending.server_received_at,
                    message: pending.message,
                },
            );
        }
    }

    fn handle_send_direct(
        &self,
        ctx: &mut ws::WebsocketContext<Self>,
        span: &mut impl opentelemetry::trace::Span,
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

        if let Some(recipient_addr) = recipient_session {
            info!(
                "Direct message {} delivered from {} to {}",
                message_id, from_agent_id, to_agent_id
            );
            recipient_addr.do_send(ServerEnvelope {
                message: WsMessage::AgentMessage {
                    message_id: message_id.clone(),
                    from_agent_id,
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
        } else {
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
        span.end();
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

                            if !self.registry.list_agents().iter().any(|a| a.agent_id == to_agent_id) {
                                Self::send_invalid(ctx, None, format!("unknown target agent {}", to_agent_id));
                                span.end();
                                return;
                            }

                            self.handle_send_direct(ctx, &mut span, from_agent_id, to_agent_id, message);
                            return;
                        }
                        WsMessage::BroadcastMessage { message } => {
                            let Some(from_agent_id) = self.assigned_agent_id().map(str::to_string) else {
                                Self::send_invalid(ctx, None, "agent must connect before broadcasting messages");
                                span.end();
                                return;
                            };

                            let recipients = self.registry.connected_sessions(&from_agent_id);
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
                                Ok((message_id, sender_session, sender_agent_id)) => {
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
                                    if let Some(sender_addr) = sender_session {
                                        sender_addr.do_send(ServerEnvelope {
                                            message: WsMessage::MessageStatus {
                                                message_id: Some(message_id),
                                                status: MessageDeliveryStatus::Acknowledged,
                                                detail: format!("agent {} acknowledged receipt", recipient_agent_id),
                                            },
                                        });
                                    }
                                }
                                Err(detail) => {
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

pub async fn ws_handler(
    req: HttpRequest,
    stream: web::Payload,
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

    let actor = WebSocketActor::new(registry.get_ref().clone(), client_ip);
    let result = ws::start(actor, &req, stream);

    span.end();
    result
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/ws", web::get().to(ws_handler));
}
