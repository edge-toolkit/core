#![cfg_attr(doc, doc = include_str!("../README.md"))]
#![expect(
    clippy::arithmetic_side_effects,
    clippy::needless_continue,
    clippy::panic,
    clippy::unwrap_used,
    clippy::wildcard_enum_match_arm,
    reason = "in-process test ws-server + ws client helpers; setup/protocol failures should fail the test fast"
)]

use std::net::{Ipv6Addr, SocketAddr, TcpListener};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use actix_web::dev::ServerHandle;
use actix_web::{App, HttpServer, web};
use edge_toolkit::ws::{AgentConnectionState, AgentSummary, ClientMessage, ServerMessage};
use et_https_listener::tls;
use et_modules_service::{ModulesConfig, configure as configure_modules};
use et_storage_service::{StorageConfig, configure as configure_storage};
use et_ws_service::admin::AdminToken;
use et_ws_service::{
    AgentSession, Backplane, BackplaneConfig, WsAgentRegistry, WsConfig, configure as configure_ws, start_backplane,
    withdraw_local_agents,
};
use futures_util::{SinkExt as _, StreamExt as _};
use tempfile::TempDir;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing_actix_web::TracingLogger;

pub mod device;
pub mod imu;
pub mod lidar;
pub mod math1;
pub mod media;

/// Actix workers per test hub; test traffic never needs more, and each worker repeats the module scan at startup.
const TEST_HUB_WORKERS: usize = 2;

/// The most sockets one actix worker holds open unless told otherwise, as actix itself defaults it.
const ACTIX_MAX_CONNECTIONS: usize = 25_000;

/// The admin bearer token every test hub accepts, so a test can drive the admin API.
pub const ADMIN_TOKEN: &str = "test-admin-token";

/// How long one [`wait_for_connected_agents`] round reads replies before asking the hub again.
const ROSTER_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// A running test server. The temporary storage directory is cleaned up on drop.
#[non_exhaustive]
pub struct TestServer {
    pub base_url: String,
    pub ws_url: String,
    /// The `/ws` URL of every address the hub listens on, [`TestServer::ws_url`] first.
    pub ws_urls: Vec<String>,
    /// The hub's own storage root; unused by a [`start_clustered`] hub, whose root the caller shares between hubs.
    pub storage_dir: TempDir,
    /// What it takes to stop the hub.
    pub control: HubControl,
}

impl TestServer {
    /// Stop this hub as `et-ws-server` does on Ctrl-C: hand its agents back to the backplane, then shut down.
    ///
    /// Unlike the server, it does not drain: open agent sockets would hold a graceful stop for actix's whole 30s
    /// shutdown timeout, and the agents have already been withdrawn by then.
    pub async fn stop(&self) {
        withdraw_local_agents(&self.control.backplane, &self.control.registry).await;
        self.control.handle.stop(false).await;
    }
}

/// The running hub's server handle and the state its shutdown hands back to the backplane.
#[non_exhaustive]
pub struct HubControl {
    handle: ServerHandle,
    backplane: Backplane,
    registry: WsAgentRegistry,
}

/// Run on each actix worker thread as it builds its app, before the hub's own services are mounted.
///
/// Mounted first because the modules service answers every path nothing earlier claimed.
pub type ConfigureHook = Arc<dyn Fn(&mut web::ServiceConfig) + Send + Sync>;

/// How a hub started by [`start_with`] is set up; the default is the hub [`start`] runs.
#[derive(Clone)]
#[non_exhaustive]
pub struct HubOptions {
    /// The hub's own settings, as `et-ws-server` reads them from `WS_*`; the admin token is always [`ADMIN_TOKEN`].
    pub ws: WsConfig,
    /// Actix worker threads, as `HTTP_WORKERS` sets them.
    pub workers: usize,
    /// The most sockets each worker holds open at once, as `HTTP_MAX_CONNECTIONS` sets it.
    ///
    /// Past it, actix stops accepting until one closes, so a hub holding more agents than this per worker raises it.
    pub max_connections: usize,
    /// Listen on `[::1]` as well as `127.0.0.1`.
    ///
    /// Each loopback address has its own range of ephemeral ports for clients connecting to it, and on macOS and
    /// Windows one range holds about 16,000 sockets, so a client holding more than that spreads them over both.
    pub ipv6: bool,
    /// Called on each worker thread with that worker's app, so a caller can mount routes of its own beside the hub's.
    pub configure: ConfigureHook,
    /// Whether every listener speaks plain HTTP or HTTPS.
    pub transport: Transport,
}

impl Default for HubOptions {
    fn default() -> Self {
        Self {
            configure: Arc::new(|_cfg| {}),
            ipv6: false,
            max_connections: ACTIX_MAX_CONNECTIONS,
            transport: Transport::Http,
            workers: TEST_HUB_WORKERS,
            ws: WsConfig::default(),
        }
    }
}

/// What a hub's listeners speak.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Transport {
    /// Plain HTTP, and `ws://` for agents.
    Http,
    /// HTTPS serving this TLS config, as `et-ws-server`'s secure listener does, and `wss://` for agents.
    Https(Box<rustls::ServerConfig>),
}

impl Transport {
    /// HTTPS with a pair self-signed for the loopback names, and a client config that trusts only that certificate.
    #[must_use]
    pub fn self_signed() -> (Self, Arc<rustls::ClientConfig>) {
        let dir = TempDir::new().unwrap();
        let (cert, key) = tls::generate_tls_certs(&dir.path().join("cert.pem"), &dir.path().join("key.pem"));
        let mut roots = rustls::RootCertStore::empty();
        roots.add(cert.clone()).unwrap();
        let client = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let server = tls::build_tls_server_config(cert, key);
        (Self::Https(Box::new(server)), Arc::new(client))
    }

    /// The URL schemes this transport's clients use: the page's, then the websocket's.
    const fn schemes(&self) -> (&'static str, &'static str) {
        match self {
            Self::Http => ("http", "ws"),
            Self::Https(_) => ("https", "wss"),
        }
    }
}

/// Start an in-process ws-server on a free port with a temporary storage directory.
///
/// Serves modules from the default module paths (same as production).
#[must_use]
pub fn start() -> TestServer {
    start_with(HubOptions::default())
}

/// Start an in-process ws-server set up as `options` says, on free ports with a temporary storage directory.
#[must_use]
pub fn start_with(options: HubOptions) -> TestServer {
    let mut listeners = vec![et_test_helpers::free_listener()];
    if options.ipv6 {
        listeners.push(TcpListener::bind((Ipv6Addr::LOCALHOST, 0)).unwrap());
    }
    let storage_dir = TempDir::new().unwrap();
    let storage_config = StorageConfig::local(storage_dir.path());
    spawn_hub(listeners, storage_dir, storage_config, options)
}

/// Start an in-process ws-server bound to a specific `port` with a temporary storage directory.
///
/// Like [`start`], but for callers that must know the port ahead of time (e.g. a fixed-port launcher a separate process
/// connects to). Panics if the port is already in use.
#[must_use]
pub fn start_on(port: u16) -> TestServer {
    let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
    let storage_dir = TempDir::new().unwrap();
    let storage_config = StorageConfig::local(storage_dir.path());
    spawn_hub(vec![listener], storage_dir, storage_config, HubOptions::default())
}

/// Start an in-process hub that shares its agents with every other hub on the NATS server at `nats_url`.
///
/// `storage_root` is the storage directory, and is meant to be handed to every hub of the cluster: it stands in for
/// the one object store (S3, say) a real scaled-out deployment points all its hubs at.
///
/// The idle timeout is off: a cluster test holds many agents that sit quiet while it services the others, and nothing
/// in a test client sends the heartbeats a real agent would.
#[must_use]
pub fn start_clustered(nats_url: &str, storage_root: &Path) -> TestServer {
    let mut options = HubOptions::default();
    options.ws.backplane = BackplaneConfig::Nats(nats_url.to_owned());
    options.ws.connection_timeout = None;
    spawn_hub(
        vec![et_test_helpers::free_listener()],
        TempDir::new().unwrap(),
        StorageConfig::local(storage_root),
        options,
    )
}

/// Run one hub on `listeners` on its own thread and actix system, returning once it is serving and on its backplane.
///
/// The first listener names [`TestServer::base_url`] and [`TestServer::ws_url`].
fn spawn_hub(
    listeners: Vec<TcpListener>,
    storage_dir: TempDir,
    storage_config: StorageConfig,
    options: HubOptions,
) -> TestServer {
    let addrs: Vec<SocketAddr> = listeners
        .iter()
        .map(|listener| listener.local_addr().unwrap())
        .collect();
    let HubOptions {
        configure,
        max_connections,
        transport,
        workers,
        ws: mut ws_config,
        ..
    } = options;
    let schemes = transport.schemes();
    ws_config.admin_token = AdminToken::try_from(ADMIN_TOKEN.to_owned()).unwrap();
    let modules_config = page_modules();
    let registry = WsAgentRegistry::default();
    let server_registry = registry.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();

    let _server_thread = std::thread::spawn(move || {
        actix_rt::System::new().block_on(async move {
            // Connected inside this system, so the NATS client's own tasks live exactly as long as the hub does.
            let backplane = Backplane::connect(&ws_config.backplane).await.unwrap();
            let delivery = start_backplane(&backplane, &server_registry, ws_config.max_frame_size)
                .await
                .unwrap();
            let backplane_data = web::Data::new(backplane.clone());
            let registry = web::Data::new(server_registry);
            let storage = web::Data::new(storage_config);
            let modules = modules_config;
            let server = HttpServer::new(move || {
                // `TracingLogger` mirrors the real ws-server's pipeline: extracts `traceparent` from incoming requests
                // so server spans are children of the caller's trace.
                App::new()
                    .wrap(TracingLogger::default())
                    .app_data(registry.clone())
                    .app_data(backplane_data.clone())
                    .app_data(storage.clone())
                    .configure(|cfg| configure(cfg))
                    .configure(|cfg| configure_ws(cfg, &ws_config))
                    .configure(|cfg| configure_storage::<AgentSession>(cfg, &storage))
                    .configure(|cfg| configure_modules(cfg, &modules))
            })
            .workers(workers)
            .max_connections(max_connections);
            let server = listeners
                .into_iter()
                .fold(server, |server, listener| match &transport {
                    Transport::Http => server.listen(listener).unwrap(),
                    Transport::Https(config) => server.listen_rustls_0_23(listener, (**config).clone()).unwrap(),
                })
                .run();
            ready_tx.send((server.handle(), backplane)).unwrap();
            server.await.unwrap();
            delivery.stop().await;
        });
    });

    // Bound before the handle is sent, so the listener already queues connections by the time this returns.
    let (handle, backplane) = ready_rx.recv_timeout(Duration::from_secs(20)).unwrap();
    let control = HubControl {
        handle,
        backplane,
        registry,
    };
    test_server(&addrs, schemes, storage_dir, control)
}

/// The default module search paths, with the page module named outright.
///
/// The server has no default for which module it serves at `/` -- that is the deployment's business, and this
/// stand-in is a deployment.
#[expect(
    clippy::single_call_fn,
    reason = "names the stand-in deployment's modules apart from starting the hub"
)]
fn page_modules() -> ModulesConfig {
    let mut modules_config = ModulesConfig::default();
    modules_config.root = "@edge-toolkit/et-ws-server-static".to_string();
    modules_config
}

/// The [`TestServer`] for a hub serving on `addrs` under the page and websocket `schemes`.
#[expect(
    clippy::single_call_fn,
    reason = "turns a running hub's addresses into the URLs its callers connect to"
)]
fn test_server(
    addrs: &[SocketAddr],
    (page_scheme, ws_scheme): (&str, &str),
    storage_dir: TempDir,
    control: HubControl,
) -> TestServer {
    let ws_urls: Vec<String> = addrs.iter().map(|addr| format!("{ws_scheme}://{addr}/ws")).collect();
    TestServer {
        base_url: format!("{page_scheme}://{}", addrs.first().unwrap()),
        ws_url: ws_urls.first().unwrap().clone(),
        ws_urls,
        storage_dir,
        control,
    }
}

/// Block until `count` agents other than the waiter itself are connected to the hub at `ws_url`.
///
/// Returns the connected peer ids from the last roster the hub sent: at least `count` of them once the wait succeeds,
/// and whoever was present when the budget ran out otherwise, so a caller that gives up can report who did come up
/// rather than only that someone did not.
///
/// Reading the roster means being an agent -- `et-list-agents` is a websocket request, not an HTTP route -- so the
/// waiter registers one of its own and filters itself out of every reply. Entries whose state is `Disconnected` are
/// filtered out too: the registry keeps listing an agent after its socket drops, so a runner that registered and then
/// exited would otherwise still read as up.
///
/// Synchronous, and owns the runtime it needs, so a plain `#[test]` can gate on hub state without taking a tokio
/// dependency of its own.
#[must_use]
pub fn wait_for_connected_agents(ws_url: &str, count: usize, budget: Duration) -> Vec<String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(poll_roster(ws_url, count, budget))
}

/// The async body of [`wait_for_connected_agents`], split out so the public helper can stay synchronous.
///
/// One socket serves the whole wait: re-asking on a fresh connection each round would leave a trail of disconnected
/// waiter entries in the registry, and a stale one still marked connected would be counted as a peer by the very filter
/// that exists to exclude it.
#[expect(
    clippy::single_call_fn,
    reason = "async body of wait_for_connected_agents; separate so the public helper stays sync"
)]
async fn poll_roster(ws_url: &str, count: usize, budget: Duration) -> Vec<String> {
    let (mut socket, self_id) = connect_agent(ws_url).await;
    let deadline = tokio::time::Instant::now() + budget;
    let mut peers = Vec::new();
    let mut ask_again = tokio::time::Instant::now();
    while tokio::time::Instant::now() < deadline {
        if tokio::time::Instant::now() >= ask_again {
            let request = serde_json::to_string(&ClientMessage::ListAgents).unwrap();
            if socket.send(Message::text(request)).await.is_err() {
                break;
            }
            ask_again = tokio::time::Instant::now() + ROSTER_POLL_INTERVAL;
        }
        let wait = deadline
            .saturating_duration_since(tokio::time::Instant::now())
            .min(ROSTER_POLL_INTERVAL);
        let Ok(frame) = tokio::time::timeout(wait, socket.next()).await else {
            continue;
        };
        match frame {
            Some(Ok(Message::Text(text))) => {
                let parsed = serde_json::from_str::<ServerMessage>(&text);
                if let Ok(ServerMessage::ListAgentsResponse { agents }) = parsed {
                    peers = connected_peers(agents, &self_id);
                    if peers.len() >= count {
                        return peers;
                    }
                }
            }
            Some(Ok(_)) => {}
            Some(Err(_)) | None => break,
        }
    }
    peers
}

/// Reduce one roster reply to the ids of the connected agents that are not the waiter.
#[expect(
    clippy::single_call_fn,
    reason = "distinct filtering step; kept out of the poll loop for readability"
)]
fn connected_peers(agents: Vec<AgentSummary>, self_id: &str) -> Vec<String> {
    agents
        .into_iter()
        .filter(|agent| agent.agent_id != self_id && agent.state == AgentConnectionState::Connected)
        .map(|agent| agent.agent_id)
        .collect()
}

/// Open a ws connection to `ws_url` and drive `et-connect` through its ack.
///
/// Returns `(stream, agent_id)` once the `et-connect-ack` has been observed. Lets a test drive the hub as a websocket
/// client.
pub async fn connect_agent(
    ws_url: &str,
) -> (
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    String,
) {
    connect_agent_as(ws_url, None).await
}

/// Open a ws connection to `ws_url` and drive `et-connect` through its ack, asking for `requested_id`.
///
/// The same as [`connect_agent`] for an agent that names itself, which is how a test puts two connections on one id.
/// `None` asks the hub to generate one, exactly as [`connect_agent`] does.
pub async fn connect_agent_as(
    ws_url: &str,
    requested_id: Option<&str>,
) -> (
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    String,
) {
    let (mut stream, _) = connect_async(ws_url).await.unwrap();
    let agent_id = requested_id.map(str::to_string);
    let connect_msg = serde_json::to_string(&ClientMessage::Connect { agent_id }).unwrap();
    stream.send(Message::text(connect_msg)).await.unwrap();

    // Bound the ack wait: a server that accepts the socket but never sends `et-connect-ack` (and never closes) must
    // fail the test fast rather than hang. Non-ack frames simply fall through and the loop reads the next.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while let Ok(Some(Ok(msg))) = tokio::time::timeout(
        deadline.saturating_duration_since(tokio::time::Instant::now()),
        stream.next(),
    )
    .await
    {
        if let Message::Text(text) = &msg
            && let Ok(ServerMessage::ConnectAck { agent_id, .. }) = serde_json::from_str::<ServerMessage>(text)
        {
            return (stream, agent_id);
        }
    }
    panic!("never received et-connect-ack within 5s");
}

/// Pull the next non-ack frame from `stream`.
///
/// Skips known protocol acks (`et-connect-ack`, `et-message-status`, `et-response`) so callers see the next "real"
/// payload.
pub async fn next_payload(
    stream: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
) -> Message {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let next = tokio::time::timeout(remaining, stream.next()).await.unwrap();
        let msg = next.unwrap().unwrap();
        match &msg {
            Message::Text(text) => {
                if serde_json::from_str::<ServerMessage>(text).is_ok_and(|parsed| {
                    matches!(
                        parsed,
                        ServerMessage::ConnectAck { .. }
                            | ServerMessage::MessageStatus { .. }
                            | ServerMessage::Response { .. }
                    )
                }) {
                    continue;
                }
                return msg;
            }
            Message::Binary(_) => return msg,
            // Ping/pong and any other control frame: skip until a real payload arrives (or the deadline elapses / the
            // stream closes, which then surfaces through the `.unwrap()` above).
            _ => continue,
        }
    }
}
