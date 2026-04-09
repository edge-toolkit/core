use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use actix::{Actor, ActorContext, AsyncContext, StreamHandler};
use actix_files::{Files, NamedFile};
use actix_web::{web, App, Error, HttpRequest, HttpResponse, HttpServer};
use actix_web_actors::ws;
use chrono::Utc;
use edge_toolkit::ws::{ConnectStatus, WsMessage};
use opentelemetry::{
    global,
    trace::{Span, Tracer},
};
use opentelemetry_sdk::trace::SdkTracerProvider as TracerProvider;
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use rustls::ServerConfig;
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

/// Maximum time the server allows a websocket connection to remain idle before closing it.
/// This should remain comfortably higher than the client's `Alive` message interval.
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(15);
/// How often the server checks whether a websocket connection has exceeded `CONNECTION_TIMEOUT`.
/// This is only the check cadence, not the allowed idle duration.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);

// Initialize OpenTelemetry
fn init_tracing() -> opentelemetry_sdk::trace::SdkTracerProvider {
    let provider = TracerProvider::builder().build();
    global::set_tracer_provider(provider.clone());
    provider
}

#[derive(Clone, Default)]
struct AgentRegistry {
    issued_agent_ids: Arc<Mutex<HashSet<String>>>,
}

// WebSocket actor for handling connections
struct WebSocketActor {
    agent_id: Option<String>,
    last_activity: Instant,
    registry: AgentRegistry,
}

impl WebSocketActor {
    fn new(registry: AgentRegistry) -> Self {
        info!("New WebSocket actor created without assigned agent_id");
        Self {
            agent_id: None,
            last_activity: Instant::now(),
            registry,
        }
    }

    fn current_agent_id(&self) -> &str {
        self.agent_id.as_deref().unwrap_or("unassigned")
    }

    fn mark_activity(&mut self) {
        self.last_activity = Instant::now();
    }

    fn start_heartbeat(&self, ctx: &mut ws::WebsocketContext<Self>) {
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

    fn assign_or_reconnect_agent(&mut self, requested_id: Option<String>) -> (String, ConnectStatus) {
        let mut issued_agent_ids = self
            .registry
            .issued_agent_ids
            .lock()
            .expect("agent registry lock poisoned");

        if let Some(requested_id) = requested_id {
            if issued_agent_ids.contains(&requested_id) {
                self.agent_id = Some(requested_id.clone());
                return (requested_id, ConnectStatus::Reconnected);
            }
        }

        let assigned_id = Uuid::now_v7().to_string();
        issued_agent_ids.insert(assigned_id.clone());
        self.agent_id = Some(assigned_id.clone());
        (assigned_id, ConnectStatus::Assigned)
    }
}

impl Actor for WebSocketActor {
    type Context = ws::WebsocketContext<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.start_heartbeat(ctx);
        info!(
            "WebSocket connection established for client: {}",
            self.current_agent_id()
        );
        let tracer = global::tracer("ws-server");
        let mut span = tracer.start("ws.connect");
        span.end();
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
                            let (assigned_id, status) = self.assign_or_reconnect_agent(agent_id);
                            info!(
                                "Connect message: requested_agent_id={:?} assigned_agent_id={} status={:?}",
                                requested_id, assigned_id, status
                            );
                            let response = WsMessage::ConnectAck {
                                agent_id: assigned_id,
                                status: status.clone(),
                            };
                            if let Ok(json) = serde_json::to_string(&response) {
                                ctx.text(json);
                                let mut sent_span = tracer.start("ws.message.sent");
                                sent_span.end();
                                info!(
                                    "WebSocket connection ready for client {} with status {:?}",
                                    self.current_agent_id(),
                                    status
                                );
                            }
                        }
                        WsMessage::Alive { timestamp } => {
                            info!("Alive message from client {} at {}", self.current_agent_id(), timestamp);
                            let response = WsMessage::Response {
                                message: format!("Alive message received at {}", Utc::now().to_rfc3339()),
                            };
                            if let Ok(json) = serde_json::to_string(&response) {
                                ctx.text(json);
                                let mut sent_span = tracer.start("ws.message.sent");
                                sent_span.end();
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
                        WsMessage::ConnectAck { .. } => {
                            warn!(
                                "Unexpected connect_ack message from client: {}",
                                self.current_agent_id()
                            );
                        }
                        WsMessage::Response { .. } => {
                            warn!("Unexpected response message from client: {}", self.current_agent_id());
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

// WebSocket endpoint handler
async fn ws_handler(
    req: HttpRequest,
    stream: web::Payload,
    registry: web::Data<AgentRegistry>,
) -> Result<HttpResponse, Error> {
    let tracer = global::tracer("ws-server");
    let mut span = tracer.start("ws.connect");

    let result = ws::start(WebSocketActor::new(registry.get_ref().clone()), &req, stream);

    span.end();
    result
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root should exist")
}

fn wasm_pkg_dir() -> PathBuf {
    workspace_root().join("services/ws-wasm-agent/pkg")
}

fn wasm_modules_dir() -> PathBuf {
    workspace_root().join("services/ws-modules")
}

fn browser_static_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("static")
}

async fn browser_index() -> Result<NamedFile, Error> {
    let path = browser_static_dir().join("index.html");
    info!("Serving browser interface page: {:?}", path);

    NamedFile::open(path).map_err(|e| {
        error!("Failed to open browser interface page: {}", e);
        actix_web::error::ErrorNotFound(e)
    })
}

async fn no_content() -> HttpResponse {
    HttpResponse::NoContent().finish()
}

// Static file handler — serves a named binary file from the ws-server static directory.
// Example: GET /files/firmware.bin  → services/ws-server/static/firmware.bin
async fn file_handler(req: HttpRequest) -> Result<NamedFile, Error> {
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

// Health check endpoint
async fn health() -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({
        "status": "healthy",
        "service": "ws-server"
    }))
}

fn tls_config() -> std::io::Result<ServerConfig> {
    let certified = rcgen::generate_simple_self_signed(vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ])
    .map_err(|e| std::io::Error::other(format!("failed to generate dev certificate: {e}")))?;

    let cert_der: CertificateDer<'static> = certified.cert.der().clone();
    let key_der = PrivatePkcs8KeyDer::from(certified.signing_key.serialize_der());

    ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der.into())
        .map_err(|e| std::io::Error::other(format!("failed to configure TLS: {e}")))
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let _provider = init_tracing();

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,ws_server=debug".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let tls_config = tls_config()?;

    info!("Starting WebSocket server on http://0.0.0.0:8080");
    info!("Starting WebSocket server on https://localhost:8443");
    info!("Serving browser assets from {:?}", browser_static_dir());
    info!("Serving wasm package from {:?}", wasm_pkg_dir());
    info!("Serving wasm modules from {:?}", wasm_modules_dir());
    info!("HTTPS uses an in-memory self-signed localhost certificate for development");
    let agent_registry = web::Data::new(AgentRegistry::default());

    HttpServer::new(move || {
        App::new()
            .app_data(agent_registry.clone())
            .route("/", web::get().to(browser_index))
            .route("/index.html", web::get().to(browser_index))
            .route("/favicon.ico", web::get().to(no_content))
            .route("/health", web::get().to(health))
            .route("/ws", web::get().to(ws_handler))
            .route("/files/{filename}", web::get().to(file_handler))
            .service(Files::new("/modules", wasm_modules_dir()).prefer_utf8(true))
            .service(Files::new("/pkg", wasm_pkg_dir()).prefer_utf8(true))
            .service(Files::new("/static", browser_static_dir()).prefer_utf8(true))
    })
    .bind(("0.0.0.0", 8080))?
    .bind_rustls_0_23(("0.0.0.0", 8443), tls_config)?
    .run()
    .await
}
