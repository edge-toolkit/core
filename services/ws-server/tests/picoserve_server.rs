//! The `no_std` ws-server over real TCP sockets, with tokio standing in for an embedded executor and network stack:
//! `/health` and `/favicon.ico` answer as the `std` server's do, and `/ws` is the hub.
#![cfg(test)]
#![recursion_limit = "256"]

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use edge_toolkit::ws::{ClientMessage, ConnectStatus, ServerMessage};
use edge_toolkit::ws_server::MessageRetention;
use et_ws_server::picoserve_server::serve;
use et_ws_server::routes::HealthResponse;
use et_ws_service::hub::Stamps;
use et_ws_service::picoserve_hub::{Hub, config};
use futures_util::{SinkExt as _, StreamExt as _};
use picoserve::time::TokioTimer;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::LocalSet;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

/// How long a test waits for an answer before failing rather than hanging.
const PATIENCE: Duration = Duration::from_secs(5);

/// Ids `id-1`, `id-2`, ... in turn, and a fixed time.
#[derive(Default)]
struct Counter(u32);

impl Stamps for Counter {
    fn new_id(&mut self) -> String {
        self.0 = self.0.saturating_add(1);
        format!("id-{}", self.0)
    }

    fn now(&mut self) -> String {
        "2026-10-30T00:00:00Z".to_owned()
    }
}

/// The server on a fresh loopback port, one task per connection as an embedder would, and its address.
#[expect(
    clippy::single_call_fn,
    reason = "the embedder's accept loop, kept apart from what the test asserts"
)]
async fn served() -> SocketAddr {
    let hub = Arc::new(Hub::new(Counter::default(), MessageRetention::All, 1024, 16));
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let _accepting = tokio::task::spawn_local(async move {
        loop {
            let (socket, peer) = listener.accept().await.unwrap();
            let hub = Arc::clone(&hub);
            let _serving = tokio::task::spawn_local(async move {
                let mut http_buffer = [0; 2048];
                let _served = serve(&hub, socket, peer.ip(), TokioTimer, &config(), &mut http_buffer).await;
            });
        }
    });
    address
}

/// The whole response to a bodiless `GET path`, headers and body, as text.
async fn get(address: SocketAddr, path: &str) -> String {
    let mut socket = TcpStream::connect(address).await.unwrap();
    let request = format!("GET {path} HTTP/1.1\r\nHost: hub\r\nConnection: close\r\n\r\n");
    socket.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::default();
    let _read = tokio::time::timeout(PATIENCE, socket.read_to_string(&mut response))
        .await
        .unwrap()
        .unwrap();
    response
}

#[tokio::test]
async fn it_serves_health_favicon_and_the_hub() {
    // Boxed because its size depends on how the build unifies features: over 16 KiB with the whole workspace, under
    // it alone, so an expectation of `clippy::large_futures` could hold in only one of the two.
    LocalSet::new()
        .run_until(Box::pin(async {
            let address = served().await;

            let health = get(address, "/health").await;
            assert!(health.starts_with("HTTP/1.1 200"));
            assert!(health.contains("Content-Type: application/json"));
            let (_head, body) = health.split_once("\r\n\r\n").unwrap();
            let body: HealthResponse = serde_json::from_str(body).unwrap();
            assert_eq!((body.status.as_str(), body.service.as_str()), ("healthy", "ws-server"));

            assert!(get(address, "/favicon.ico").await.starts_with("HTTP/1.1 204"));
            assert!(get(address, "/elsewhere").await.starts_with("HTTP/1.1 404"));

            let (mut agent, _response) = connect_async(format!("ws://{address}/ws")).await.unwrap();
            let connect = ClientMessage::Connect { agent_id: None };
            let frame = Message::text(serde_json::to_string(&connect).unwrap());
            agent.send(frame).await.unwrap();
            let ack = tokio::time::timeout(PATIENCE, agent.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert!(matches!(
                ServerMessage::from_text_frame(ack.to_text().unwrap()).unwrap(),
                ServerMessage::ConnectAck { status: ConnectStatus::Assigned, agent_id } if agent_id == "id-1"
            ));
        }))
        .await;
}
