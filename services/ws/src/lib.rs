#![cfg_attr(doc, doc = include_str!("../README.md"))]
#![cfg_attr(feature = "docs", doc = "## Feature flags")]
#![cfg_attr(feature = "docs", doc = document_features::document_features!())]

use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::time::Duration;

use actix_web::{Error, HttpRequest, HttpResponse, web};
use bytes::Bytes;
use edge_toolkit::ws::{CloudEvent, ServerMessage, client_event_type};
use edge_toolkit::ws_server::{AgentRecord, AgentRegistry, PendingDirectMessage, RegistryError};
use opentelemetry::{
    global,
    metrics::{Counter, UpDownCounter},
    trace::{Span as _, Tracer as _},
};
use serde::Deserialize;
use serde_default::DefaultFromSerde;
use serde_inline_default::serde_inline_default;
use tokio::sync::mpsc::{self, UnboundedSender};
use tracing::{info, warn};

mod connection;

use self::connection::Connection;

/// Default idle timeout before the hub closes a quiet connection.
pub const DEFAULT_CONNECTION_TIMEOUT: Duration = Duration::from_secs(15);
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);

/// Default max WebSocket frame size (64 MiB).
///
/// Large binary payloads fanned out via default broadcast (e.g. tensors) easily blow past actix-ws's 64 KiB default.
/// Override via the `WS_MAX_FRAME_SIZE` env var, as a human byte size (`serde-env` translates `[ws] max_frame_size`
/// to `WS_MAX_FRAME_SIZE`).
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

/// Runtime knobs for the WebSocket hub.
///
/// Populated by `serde-env` in `et-ws-server::main`, then handed to `configure`.
#[serde_inline_default]
#[derive(Clone, Debug, DefaultFromSerde, Deserialize)]
#[cfg_attr(feature = "env-schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct WsConfig {
    /// Largest single WebSocket frame the hub will accept.
    ///
    /// Frames above this are dropped by actix-ws before they reach the handler, so callers shipping big tensors / blobs
    /// need to raise it above their payload size. `WS_MAX_FRAME_SIZE` takes a human byte size (e.g. `64MiB`, `64MB`,
    /// `512KiB`) or a plain byte count; unset defaults to 64 MiB.
    #[serde(default = "default_max_frame_size", deserialize_with = "deserialize_byte_size")]
    #[cfg_attr(feature = "env-schema", schemars(with = "String", extend("default" = "64MiB")))]
    pub max_frame_size: usize,

    /// Idle period before the hub closes a connection, as a humantime duration (e.g. `15s`, `1m30s`).
    ///
    /// Unset defaults to 15s; `none`/`off`/`disabled` turns the idle timeout off (the hub never closes a connection for
    /// inactivity), which suits a frontend that sits idle.
    #[serde(
        default = "default_connection_timeout",
        deserialize_with = "edge_toolkit::config::deserialize_optional_humantime"
    )]
    #[cfg_attr(feature = "env-schema", schemars(with = "Option<String>", extend("default" = "15s")))]
    pub connection_timeout: Option<Duration>,
}

const fn default_max_frame_size() -> usize {
    DEFAULT_MAX_FRAME_SIZE
}

/// Parse `WS_MAX_FRAME_SIZE` as a human byte size (e.g. `64MiB`, `64MB`, `512KiB`) or a plain byte count.
///
/// Delegates the parsing to `bytesize`.
fn deserialize_byte_size<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // `bytesize`'s own `Deserialize` parses the human size ("64MiB", "512KiB", a bare byte count); its `D::Error`
    // cascades through `?`, no `.map_err`. `usize::try_from` only narrows on 32-bit hosts, where clamping a frame cap
    // to `usize::MAX` is harmless.
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
        agent_id, event.event_type, event.source, event.id, event.time, details
    );
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
