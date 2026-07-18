#![expect(
    clippy::arithmetic_side_effects,
    clippy::needless_continue,
    clippy::panic,
    clippy::unwrap_used,
    clippy::wildcard_enum_match_arm,
    reason = "in-process test ws-server + ws client helpers; setup/protocol failures should fail the test fast"
)]

use std::time::Duration;

use actix_web::{App, HttpServer, web};
use edge_toolkit::ws::{ClientMessage, ServerMessage};
use et_modules_service::{ModulesConfig, configure as configure_modules};
use et_storage_service::{StorageConfig, configure as configure_storage};
use et_ws_service::{AgentSession, WsAgentRegistry, WsConfig, configure as configure_ws};
use futures_util::{SinkExt as _, StreamExt as _};
use tempfile::TempDir;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing_actix_web::TracingLogger;

/// A running test server. The temporary storage directory is cleaned up on drop.
#[non_exhaustive]
pub struct TestServer {
    pub base_url: String,
    pub ws_url: String,
    pub storage_dir: TempDir,
}

/// Start an in-process ws-server on a free port with a temporary storage directory.
///
/// Serves modules from the default module paths (same as production).
#[must_use]
pub fn start() -> TestServer {
    start_on(et_test_helpers::reserve_port())
}

/// Start an in-process ws-server bound to a specific `port` with a temporary storage directory.
///
/// Like [`start`], but for callers that must know the port ahead of time (e.g. a fixed-port launcher a separate
/// process connects to). Panics if the port is already in use.
#[must_use]
pub fn start_on(port: u16) -> TestServer {
    let storage_dir = TempDir::new().unwrap();
    let storage_path = storage_dir.path().to_path_buf();

    let storage_config = StorageConfig::new(storage_path);
    let modules_config = ModulesConfig::default();
    let addr = format!("127.0.0.1:{port}");

    let _server_thread = std::thread::spawn(move || {
        actix_rt::System::new().block_on(async move {
            let registry = web::Data::new(WsAgentRegistry::default());
            let storage = web::Data::new(storage_config);
            let modules = modules_config;
            let ws_config = WsConfig::default();
            HttpServer::new(move || {
                // `TracingLogger` mirrors the real ws-server's pipeline:
                // extracts `traceparent` from incoming requests so server
                // spans are children of the caller's trace.
                App::new()
                    .wrap(TracingLogger::default())
                    .app_data(registry.clone())
                    .app_data(storage.clone())
                    .configure(|cfg| configure_ws(cfg, &ws_config))
                    .configure(|cfg| configure_storage::<AgentSession>(cfg, &storage))
                    .configure(|cfg| configure_modules(cfg, &modules))
            })
            .bind(&addr)
            .unwrap()
            .run()
            .await
            .unwrap();
        });
    });

    for _ in 0_u32..50 {
        if std::net::TcpStream::connect(format!("127.0.0.1:{port}")).is_ok() {
            return TestServer {
                base_url: format!("http://127.0.0.1:{port}"),
                ws_url: format!("ws://127.0.0.1:{port}/ws"),
                storage_dir,
            };
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("test ws-server did not start within 5 seconds on port {port}");
}

/// Open a ws connection to `ws_url`, send `et-connect`, and return `(stream, agent_id)` once the
/// `et-connect-ack` has been observed. Lets a test drive the hub as a websocket client.
pub async fn connect_agent(
    ws_url: &str,
) -> (
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    String,
) {
    let (mut stream, _) = connect_async(ws_url).await.unwrap();
    let connect_msg = serde_json::to_string(&ClientMessage::Connect { agent_id: None }).unwrap();
    stream.send(Message::text(connect_msg)).await.unwrap();

    // Bound the ack wait: a server that accepts the socket but never sends `et-connect-ack` (and never closes)
    // must fail the test fast rather than hang. Non-ack frames simply fall through and the loop reads the next.
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

/// Pull the next frame from `stream`, skipping known protocol acks (`et-connect-ack`,
/// `et-message-status`, `et-response`) so callers see the next "real" payload.
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
            // Ping/pong and any other control frame: skip until a real payload arrives (or the deadline
            // elapses / the stream closes, which then surfaces through the `.unwrap()` above).
            _ => continue,
        }
    }
}
