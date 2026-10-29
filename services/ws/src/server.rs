//! The actix-web hub: its configuration, its registry file, and the `/ws` endpoint each agent connects to.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use actix_web::{Error, HttpRequest, HttpResponse, web};
use chrono::Utc;
pub use edge_toolkit::ws_server::MessageRetention;
use edge_toolkit::ws_server::{
    AgentRecord, AgentRegistry, IpBlocklist, PendingDirectMessage, RegistryError, read_registry_file,
};
use serde::Deserialize;
use serde_default::DefaultFromSerde;
use serde_inline_default::serde_inline_default;
use tokio::sync::mpsc::{self, UnboundedSender};
use tracing::warn;
use uuid::Uuid;

use crate::admin::{self, AdminToken};
#[cfg(feature = "nats")]
use crate::backplane::{BackplaneConfig, backplane_of};
use crate::connection::Connection;
use crate::hub::{Link, SessionMessage, Stamps};
use crate::telemetry::HubSpan;

/// Default idle timeout before the hub closes a quiet connection.
pub const DEFAULT_CONNECTION_TIMEOUT: Duration = Duration::from_secs(15);
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);

/// Default max WebSocket frame size (64 MiB).
///
/// Large binary payloads fanned out via default broadcast (e.g. tensors) easily blow past actix-ws's 64 KiB default.
/// Override via the `WS_MAX_FRAME_SIZE` env var, as a human byte size (`serde-env` translates `[ws] max_frame_size`
/// to `WS_MAX_FRAME_SIZE`).
pub const DEFAULT_MAX_FRAME_SIZE: usize = 64 * 1024 * 1024;

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
    #[cfg(feature = "nats")]
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

pub type AgentSession = UnboundedSender<SessionMessage>;
pub type WsAgentRegistry = AgentRegistry<AgentSession>;

/// The hub's ids and times: version 7 UUIDs, and the wall clock.
pub(crate) struct SystemStamps;

impl Stamps for SystemStamps {
    fn new_id(&mut self) -> String {
        Uuid::now_v7().to_string()
    }

    fn now(&mut self) -> String {
        Utc::now().to_rfc3339()
    }
}

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
    let _span = HubSpan::start("ws.connect");

    // The socket peer only, never `Forwarded` / `X-Forwarded-For`: those are client-supplied text that would reach
    // the logs unvalidated. Every listener is TCP, so a request without a peer means the server was bound wrongly.
    let client_ip = req
        .peer_addr()
        .ok_or_else(|| actix_web::error::ErrorInternalServerError("websocket request has no peer address"))?
        .ip();
    if registry.blocklist.is_blocked(client_ip) {
        warn!("Refused a WebSocket connection from blocked IP {client_ip}");
        return Ok(HttpResponse::Forbidden().finish());
    }

    let (response, session, msg_stream) = actix_ws::handle(&req, body)?;
    let stream = msg_stream
        .max_frame_size(config.max_frame_size)
        .aggregate_continuations();

    let (tx, rx) = mpsc::unbounded_channel::<SessionMessage>();
    let link = Link::new(client_ip, tx, config.message_retention);
    let conn = Connection::new(registry.get_ref().clone(), link, session, config.connection_timeout);
    #[cfg(feature = "nats")]
    let conn = conn.with_backplane(backplane_of(&req));

    let _join = actix_web::rt::spawn(async move {
        conn.run(stream, rx).await;
    });

    Ok(response)
}

pub fn configure(cfg: &mut web::ServiceConfig, config: &WsConfig) {
    let _routed = cfg
        .app_data(web::Data::new(config.clone()))
        .route("/ws", web::get().to(ws_handler))
        .route("/agents/{agent_id}", web::delete().to(admin::evict_agent))
        .route("/blocked-ips/{ip}", web::put().to(admin::block_ip));
}
