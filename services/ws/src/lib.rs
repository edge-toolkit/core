#![cfg_attr(doc, doc = include_str!("../README.md"))]
#![cfg_attr(feature = "docs", doc = "## Feature flags")]
#![cfg_attr(feature = "docs", doc = document_features::document_features!())]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::LazyLock;
use std::time::Duration;

use actix_web::{Error, HttpRequest, HttpResponse, web};
use bytes::Bytes;
use edge_toolkit::ws::{CloudEvent, ServerMessage, client_event_type};
pub use edge_toolkit::ws_server::MessageRetention;
use edge_toolkit::ws_server::{
    AgentRecord, AgentRegistry, IpBlocklist, PendingDirectMessage, RegistryError, read_registry_file,
};
pub use et_hub_backplane::{Backplane, BackplaneConfig, BackplaneError};
use et_hub_backplane::{Frame, Inbound};
use futures_util::{Stream, StreamExt as _};
use opentelemetry::{
    global,
    metrics::{Counter, UpDownCounter},
    trace::{Span as _, Tracer as _},
};
use serde::Deserialize;
use serde_default::DefaultFromSerde;
use serde_inline_default::serde_inline_default;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::{info, warn};

pub mod admin;
mod connection;

use self::admin::AdminToken;
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

    /// What the hub shares its agents through: `local` (the default) or a `nats://` / `tls://` URL.
    ///
    /// `local` keeps every agent in this process, persisted to the `WS_AGENT_REGISTRY` file. A NATS URL names a
    /// server with `JetStream`, and every hub pointed at it shares one agent directory, one mailbox of unacknowledged
    /// direct messages, and delivery between them, so hubs scale out behind a load balancer: an agent on one hub
    /// lists, messages, broadcasts to and acknowledges agents on any other, and may reconnect to a different hub
    /// without losing what was queued for it. The directory and mailbox live in `JetStream`, so the registry file is
    /// neither read nor written in this mode.
    #[serde(default)]
    #[cfg_attr(feature = "env-schema", schemars(with = "String", extend("default" = "local")))]
    pub backplane: BackplaneConfig,

    /// Which unacknowledged direct messages a recipient holds: `latest-per-sender` (the default) or `all`.
    ///
    /// `latest-per-sender` keeps only the newest message from each sender, replacing the earlier one, so a recipient
    /// holds at most one message per sender and the hub's memory for them is bounded. `all` keeps every message until
    /// it is acknowledged, with no bound on how many pile up for a recipient that stays away.
    #[serde(default)]
    #[cfg_attr(
        feature = "env-schema",
        schemars(with = "String", extend("default" = "latest-per-sender"))
    )]
    pub message_retention: MessageRetention,

    /// The YAML file a local hub loads its agent registry from at startup and saves it to on shutdown.
    #[serde_inline_default(PathBuf::from("registry.yaml"))]
    #[cfg_attr(feature = "env-schema", schemars(extend("default" = "registry.yaml")))]
    pub agent_registry: PathBuf,

    /// Bearer token authorizing the admin API: `PUT /blocked-ips/{ip}` and `DELETE /agents/{agent_id}`.
    ///
    /// Unset disables the admin API, which then refuses every request with 403; an empty value is rejected.
    #[serde(default)]
    #[cfg_attr(feature = "env-schema", schemars(with = "Option<String>"))]
    pub admin_token: AdminToken,
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

impl From<Frame> for SessionMessage {
    fn from(value: Frame) -> Self {
        match value {
            Frame::Text(text) => Self::Text(text),
            Frame::Binary(bytes) => Self::Binary(bytes),
        }
    }
}

pub type AgentSession = UnboundedSender<SessionMessage>;
pub type WsAgentRegistry = AgentRegistry<AgentSession>;

// Deserialize using a session-less record type, then convert.
#[derive(serde::Deserialize)]
struct BareRecord {
    state: edge_toolkit::ws::AgentConnectionState,
    last_known_ip: std::net::IpAddr,
    #[serde(default)]
    pending_direct_messages: BTreeMap<String, PendingDirectMessage>,
}

/// Load a registry from disk. Sessions are not persisted, so they are initialised to `None`.
pub fn load_registry(path: &std::path::Path) -> Result<WsAgentRegistry, RegistryError> {
    let file = read_registry_file::<BareRecord>(path)?;
    let agents = file
        .agents
        .into_iter()
        .map(|(id, record)| {
            (
                id,
                AgentRecord::new(record.state, record.last_known_ip, None)
                    .with_pending_direct_messages(record.pending_direct_messages),
            )
        })
        .collect();
    Ok(WsAgentRegistry::from_agents(agents).with_blocklist(IpBlocklist::from_ips(file.blocked_ips)))
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

/// The backplane the app serving `req` joined.
///
/// An app that never joined one is a local hub.
fn backplane_of(req: &HttpRequest) -> Backplane {
    req.app_data::<web::Data<Backplane>>()
        .map_or(Backplane::Local, |backplane| backplane.get_ref().clone())
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

    // The socket peer only, never `Forwarded` / `X-Forwarded-For`: those are client-supplied text that would reach
    // the logs unvalidated. Every listener is TCP, so a request without a peer means the server was bound wrongly.
    let client_ip = req
        .peer_addr()
        .ok_or_else(|| actix_web::error::ErrorInternalServerError("websocket request has no peer address"))?
        .ip();
    if registry.blocklist.is_blocked(client_ip) {
        warn!("Refused a WebSocket connection from blocked IP {client_ip}");
        span.end();
        return Ok(HttpResponse::Forbidden().finish());
    }

    let (response, session, msg_stream) = actix_ws::handle(&req, body)?;
    let stream = msg_stream
        .max_frame_size(config.max_frame_size)
        .aggregate_continuations();

    let (tx, rx) = mpsc::unbounded_channel::<SessionMessage>();
    let conn = Connection::new(
        registry.get_ref().clone(),
        backplane_of(&req),
        client_ip,
        session,
        tx,
        config.connection_timeout,
        config.message_retention,
    );

    let _join = actix_web::rt::spawn(async move {
        conn.run(stream, rx).await;
    });

    span.end();
    Ok(response)
}

/// Start delivering what other hubs send this hub's agents; a no-op for a `Local` backplane.
///
/// Returns once the subscription is live and the shared blocklist is in `registry`, so nothing another hub sends or
/// blocks afterwards can be missed. Delivery then runs on a task of the current actix runtime until the
/// [`BackplaneDelivery`] returned is stopped.
///
/// `max_frame_size` is the largest frame this hub accepts. A broker whose `max_payload` is smaller cannot carry such a
/// frame to another hub, so that mismatch is warned about here, once, rather than left to surface as a failed relay.
pub async fn start_backplane(
    backplane: &Backplane,
    registry: &WsAgentRegistry,
    max_frame_size: usize,
) -> Result<BackplaneDelivery, BackplaneError> {
    let Backplane::Nats(nats) = backplane else {
        return Ok(BackplaneDelivery(DeliveryState::Local));
    };
    warn_if_broker_payload_too_small(nats.max_payload(), max_frame_size);
    let inbox = Box::pin(nats.subscribe().await?);
    for ip in nats.blocked_ips().await? {
        let _newly = registry.blocklist.block(ip);
    }
    let (stop, stopped) = oneshot::channel::<()>();
    let task = actix_web::rt::spawn(deliver_until_stopped(inbox, stopped, registry.clone()));
    info!("Hub {} joined the NATS backplane", nats.hub_id());
    Ok(BackplaneDelivery(DeliveryState::Nats { stop, task }))
}

/// Say, at startup, when the broker cannot carry a frame as large as this hub accepts.
#[expect(
    clippy::single_call_fn,
    reason = "the startup check of start_backplane, kept out of its body"
)]
fn warn_if_broker_payload_too_small(max_payload: usize, max_frame_size: usize) {
    if max_payload < max_frame_size {
        let effect = "a larger frame reaches only the agents of the hub it arrived at";
        warn!("NATS max_payload {max_payload} < WS_MAX_FRAME_SIZE {max_frame_size}; raise the broker's: {effect}");
    }
}

/// Hand each frame `inbox` yields to the local agents it is for, until `stopped` fires or the inbox ends.
#[expect(
    clippy::integer_division_remainder_used,
    clippy::single_call_fn,
    reason = "the delivery task start_backplane spawns; tokio::select! uses % internally"
)]
async fn deliver_until_stopped(
    mut inbox: Pin<Box<impl Stream<Item = Inbound>>>,
    mut stopped: oneshot::Receiver<()>,
    registry: WsAgentRegistry,
) {
    loop {
        tokio::select! {
            _stop = &mut stopped => break,
            inbound = inbox.next() => match inbound {
                Some(inbound) => deliver_inbound(&registry, inbound),
                None => break,
            },
        }
    }
}

/// Hand one frame from another hub to the local agents it is for.
#[expect(
    clippy::single_call_fn,
    reason = "the per-frame step of the delivery loop, kept out of the select!"
)]
fn deliver_inbound(registry: &WsAgentRegistry, inbound: Inbound) {
    match inbound {
        Inbound::Direct { to_agent_id, frame } => {
            if let Some(session) = registry.agent_session(&to_agent_id) {
                let _sent = session.send(frame.into());
            }
        }
        Inbound::Broadcast { from_agent_id, frame } => {
            for (_, session) in registry.connected_sessions(&from_agent_id) {
                let _sent = session.send(frame.clone().into());
            }
        }
        Inbound::Blocked { ip } => {
            if registry.blocklist.block(ip) {
                info!("Blocked IP {ip} on the backplane's word");
            }
        }
    }
}

/// This hub's delivery of what other hubs send its agents, running until [`BackplaneDelivery::stop`].
#[must_use = "a delivery left to be dropped with the runtime aborts the process on Windows; call stop"]
#[non_exhaustive]
pub struct BackplaneDelivery(DeliveryState);

/// Whether there is a delivery task to stop.
enum DeliveryState {
    Local,
    Nats {
        stop: oneshot::Sender<()>,
        task: JoinHandle<()>,
    },
}

impl BackplaneDelivery {
    /// Stop delivering, and wait until the backplane subscriptions are closed.
    ///
    /// Call it while the runtime is still running. async-nats unsubscribes by spawning a task when a subscription is
    /// dropped, so a subscription left to be dropped with the runtime panics with
    ///
    ///     there is no reactor running, must be called from the context of a Tokio 1.x runtime
    ///
    /// from `async-nats-0.50.0/src/lib.rs:1424`, and on Windows that panic in a destructor aborts the process with
    /// `0xc0000409`. Observed on commit
    /// <https://github.com/edge-toolkit/core/commit/92a41747e7f6079494da3e6e351a2f2dba3979b2> at
    /// <https://github.com/edge-toolkit/core/actions/runs/39220980233/job/117955940089>.
    pub async fn stop(self) {
        if let DeliveryState::Nats { stop, task } = self.0 {
            // Either the task already ended or it is told to now; both leave only the join to wait for.
            let _sent = stop.send(());
            if let Err(error) = task.await {
                warn!("The backplane delivery task ended abnormally: {error}");
            }
        }
    }
}

/// Mark every agent connected to this hub as gone from the shared directory, ahead of the hub shutting down.
///
/// A stopping server drops its connection tasks rather than letting each one run its own disconnect, so without this
/// the directory would go on placing those agents on a hub that is no longer there. A no-op for `Local`.
pub async fn withdraw_local_agents(backplane: &Backplane, registry: &WsAgentRegistry) {
    let Backplane::Nats(nats) = backplane else {
        return;
    };
    for (agent_id, _session) in registry.connected_sessions("") {
        if let Err(error) = nats.withdraw(&agent_id).await {
            warn!("Failed to withdraw agent {agent_id} from the backplane: {error}");
        }
    }
}

pub fn configure(cfg: &mut web::ServiceConfig, config: &WsConfig) {
    let _routed = cfg
        .app_data(web::Data::new(config.clone()))
        .route("/ws", web::get().to(ws_handler))
        .route("/agents/{agent_id}", web::delete().to(admin::evict_agent))
        .route("/blocked-ips/{ip}", web::put().to(admin::block_ip));
}
