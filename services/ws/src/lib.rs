use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use actix_web::{Error, HttpRequest, HttpResponse, web};
use actix_ws::{AggregatedMessage, AggregatedMessageStream, CloseCode, CloseReason, Session};
use bytes::Bytes;
use chrono::Utc;
use edge_toolkit::ws::{ClientMessage, ConnectStatus, MessageDeliveryStatus, MessageScope, ServerMessage};
use edge_toolkit::ws_server::{AgentRecord, AgentRegistry, PendingDirectMessage, RegistryError};
use futures_util::StreamExt as _;
use opentelemetry::{
    global,
    metrics::{Counter, UpDownCounter},
    trace::{Span, Tracer as _},
};
use serde::Deserialize;
use serde_default::DefaultFromSerde;
use serde_inline_default::serde_inline_default;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tracing::{error, info, warn};
use uuid::Uuid;

/// Default idle timeout before the hub closes a quiet connection.
pub const DEFAULT_CONNECTION_TIMEOUT: Duration = Duration::from_secs(15);
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);

/// Default max WebSocket frame size (64 MiB).
///
/// Large binary payloads fanned out via default broadcast (e.g. tensors)
/// easily blow past actix-ws's 64 KiB default. Override via the
/// `WS_MAX_FRAME_SIZE` env var, as a human byte size (`serde-env` translates
/// `[ws] max_frame_size` to `WS_MAX_FRAME_SIZE`).
pub const DEFAULT_MAX_FRAME_SIZE: usize = 64 * 1024 * 1024;

// Hub metrics, recorded through the global meter `et_otlp::init` installs (mirrors the `global::tracer` use above).
// Built lazily on first use -- by then the meter provider is set -- and cached for the process.
static MESSAGES_RECEIVED: LazyLock<Counter<u64>> = LazyLock::new(|| {
    global::meter("ws-server")
        .u64_counter("et_ws.messages.received")
        .with_description("Inbound WebSocket frames the hub has handled")
        .build()
});
static ACTIVE_CONNECTIONS: LazyLock<UpDownCounter<i64>> = LazyLock::new(|| {
    global::meter("ws-server")
        .i64_up_down_counter("et_ws.connections.active")
        .with_description("Currently-open WebSocket connections")
        .build()
});

/// Runtime knobs for the WebSocket hub. Populated by `serde-env` in
/// `et-ws-server::main`, then handed to `configure`.
#[serde_inline_default]
#[derive(Clone, Debug, DefaultFromSerde, Deserialize)]
#[non_exhaustive]
pub struct WsConfig {
    /// Largest single WebSocket frame the hub will accept. Frames above this
    /// are dropped by actix-ws before they reach the handler, so callers
    /// shipping big tensors / blobs need to raise it above their payload size.
    /// `WS_MAX_FRAME_SIZE` takes a human byte size (e.g. `64MiB`, `64MB`,
    /// `512KiB`) or a plain byte count; unset defaults to 64 MiB.
    #[serde(default = "default_max_frame_size", deserialize_with = "deserialize_byte_size")]
    pub max_frame_size: usize,

    /// Idle period before the hub closes a connection, as a humantime
    /// duration (e.g. `15s`, `1m30s`). Unset defaults to 15s;
    /// `none`/`off`/`disabled` turns the idle timeout off (the hub never closes
    /// a connection for inactivity), which suits a frontend that sits idle.
    #[serde(
        default = "default_connection_timeout",
        deserialize_with = "edge_toolkit::config::deserialize_optional_humantime"
    )]
    pub connection_timeout: Option<Duration>,
}

const fn default_max_frame_size() -> usize {
    DEFAULT_MAX_FRAME_SIZE
}

/// Parse `WS_MAX_FRAME_SIZE` as a human byte size (e.g. `64MiB`, `64MB`,
/// `512KiB`) or a plain byte count, via `bytesize`.
fn deserialize_byte_size<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // `bytesize`'s own `Deserialize` parses the human size ("64MiB", "512KiB",
    // a bare byte count); its `D::Error` cascades through `?`, no `.map_err`.
    // `usize::try_from` only narrows on 32-bit hosts, where clamping a frame
    // cap to `usize::MAX` is harmless.
    let size = <bytesize::ByteSize as serde::Deserialize>::deserialize(deserializer)?;
    Ok(usize::try_from(size.as_u64()).unwrap_or(usize::MAX))
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "serde default fn must return the field type Option<Duration>; the default is always Some"
)]
const fn default_connection_timeout() -> Option<Duration> {
    Some(DEFAULT_CONNECTION_TIMEOUT)
}

/// Outbound envelope written to an agent's websocket session.
///
/// `Json` is the normal path for protocol messages. `Text` and `Binary` carry
/// payloads the server forwards verbatim -- used by the hub-style fallback
/// that broadcasts unrecognised frames to every other connected agent.
#[derive(Debug, Clone)]
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
    let yaml = fs_err::read_to_string(path)?;
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
    /// Idle timeout for this connection, or `None` to never time out.
    idle_timeout: Option<Duration>,
}

impl Connection {
    #[expect(clippy::single_call_fn, reason = "inherent constructor; used once by ws_handler")]
    fn new(
        registry: WsAgentRegistry,
        client_ip: String,
        session: Session,
        outbox: AgentSession,
        idle_timeout: Option<Duration>,
    ) -> Self {
        info!("New WebSocket connection for client IP {}", client_ip);
        Self {
            agent_id: None,
            last_activity: Instant::now(),
            client_ip,
            registry,
            session,
            outbox,
            idle_timeout,
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

    /// This connection's agent id, auto-registering one on first use.
    ///
    /// A client that never sends `et-connect` -- e.g. a frontend that speaks
    /// only its own protocol -- still joins the hub relay: its first
    /// unrecognised frame implicitly registers the session, so the frame is
    /// broadcast to the other agents and this client then receives the
    /// relayed replies. et-protocol agents send `et-connect` first, so they
    /// are already assigned by the time they relay and this is a no-op.
    fn ensure_assigned_agent(&mut self) -> String {
        if let Some(id) = self.assigned_agent_id() {
            return id.to_string();
        }
        let (id, _status) = self.assign_or_reconnect_agent(None);
        info!("Auto-registered relay client {} as agent {id}", self.client_ip);
        id
    }

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
        for pending in self.registry.pending_messages_for(&agent_id) {
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
        let server_received_at = Utc::now().to_rfc3339();
        let Some((pending, recipient_session)) = self.registry.queue_direct(
            Uuid::now_v7().to_string(),
            &from_agent_id,
            &to_agent_id,
            server_received_at,
            message,
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
            drop(recipient.send(SessionMessage::Json(ServerMessage::AgentMessage {
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
    /// the sender. Used by the `RelayText` / `RelayBinary` arms of the
    /// inbound dispatcher when a frame doesn't carry our `et-*` tag.
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
            drop(recipient.send(SessionMessage::Binary(bytes.clone())));
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
                let from_agent_id = self.ensure_assigned_agent();
                self.broadcast_raw_binary(&from_agent_id, &bytes);
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
                            let agents = self.registry.list_agents();
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

                            // Unknown / departed recipients are handled by handle_send_direct's queue miss
                            // below -- a single place that answers Invalid -- so there is no pre-check here.
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
                            for (_, recipient) in &recipients {
                                drop(recipient.send(SessionMessage::Json(ServerMessage::AgentMessage {
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
                        ClientMessage::MessageAck { message_id } => {
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
                                        drop(sender.send(SessionMessage::Json(ServerMessage::MessageStatus {
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
                        ClientMessage::ClientEvent {
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
                        ClientMessage::RelayText { content } => {
                            let from_agent_id = self.ensure_assigned_agent();
                            self.broadcast_raw_text(&from_agent_id, &content);
                        }
                        ClientMessage::RelayBinary { content } => {
                            // A binary tungstenite frame is dispatched
                            // by the outer `AggregatedMessage::Binary`
                            // arm. If a client explicitly sends
                            // `{"type":"et-relay-binary",...}` as a
                            // text frame, honour it by relaying the
                            // payload as a binary frame.
                            let from_agent_id = self.ensure_assigned_agent();
                            self.broadcast_raw_binary(&from_agent_id, &Bytes::from(content));
                        }
                    },
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
        ACTIVE_CONNECTIONS.add(1, &[]);

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
                    if let Some(timeout) = self.idle_timeout {
                        let idle_for = Instant::now().saturating_duration_since(self.last_activity);
                        if idle_for > timeout {
                            warn!(
                                "WebSocket connection timed out for client {} after {:?} of inactivity",
                                self.current_agent_id(),
                                idle_for
                            );
                            let _closed: Result<(), actix_ws::Closed> = self.session.clone().close(Some(CloseReason {
                                code: CloseCode::Policy,
                                description: Some(format!(
                                    "connection timed out after {timeout:?} of inactivity"
                                )),
                            })).await;
                            break;
                        }
                    }
                }
            }
        }

        ACTIVE_CONNECTIONS.add(-1, &[]);
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

    let (tx, rx) = mpsc::unbounded_channel::<SessionMessage>();
    let conn = Connection::new(
        registry.get_ref().clone(),
        client_ip,
        session,
        tx,
        config.connection_timeout,
    );

    let _join = actix_web::rt::spawn(async move {
        conn.run(stream, rx).await;
    });

    span.end();
    Ok(response)
}

pub fn configure(cfg: &mut web::ServiceConfig, config: &WsConfig) {
    let _routed = cfg
        .app_data(web::Data::new(config.clone()))
        .route("/ws", web::get().to(ws_handler));
}
