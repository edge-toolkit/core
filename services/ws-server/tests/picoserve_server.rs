//! The `no_std` ws-server over real TCP sockets, with tokio standing in for an embedded executor and network stack:
//! `/health` and `/favicon.ico` answer as the `std` server's do, `/ws` is the hub, and the modules built into the
//! binary are served from `/modules` and `/`.
#![cfg(test)]
#![recursion_limit = "256"]

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use edge_toolkit::ws::{ClientMessage, ConnectStatus, ServerMessage};
use edge_toolkit::ws_server::MessageRetention;
use et_ws_server::picoserve_modules::{Module, ModuleFile, Modules};
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

/// A page served at `/`, a scoped module beside it, and a package redirected to a CDN.
static MODULES: Modules = Modules::new(
    &[
        Module::new(
            "page",
            &[
                ("index.html", ModuleFile::new("text/html", b"<p>page</p>", "index")),
                ("app.js", ModuleFile::new("text/javascript", b"app", "app")),
            ],
        ),
        Module::new(
            "@scope/math",
            &[
                ("package.json", ModuleFile::new("application/json", b"{}", "package")),
                ("pkg/math.wasm", ModuleFile::new("application/wasm", b"\0asm", "wasm")),
            ],
        ),
    ],
    "page",
    &["big"],
    "https://cdn.example/npm",
);

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

/// The server of `modules` on a fresh loopback port, one task per connection as an embedder would, and its address.
async fn served(modules: &'static Modules) -> SocketAddr {
    let hub = Arc::new(Hub::new(Counter::default(), MessageRetention::All, 1024, 16));
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let _accepting = tokio::task::spawn_local(async move {
        loop {
            let (socket, peer) = listener.accept().await.unwrap();
            let hub = Arc::clone(&hub);
            let _serving = tokio::task::spawn_local(async move {
                let mut http_buffer = [0; 2048];
                let _served = serve(
                    &hub,
                    modules,
                    socket,
                    peer.ip(),
                    TokioTimer,
                    &config(),
                    &mut http_buffer,
                )
                .await;
            });
        }
    });
    address
}

/// The whole response to a bodiless `GET path`, headers and body, as text.
async fn get(address: SocketAddr, path: &str) -> String {
    request(address, "GET", path).await
}

/// The whole response to a bodiless `method path`, headers and body, as text.
async fn request(address: SocketAddr, method: &str, path: &str) -> String {
    send(address, &format!("{method} {path} HTTP/1.1\r\nHost: hub\r\n")).await
}

/// The whole response to `head`, a request line and headers to which the closing ones are added.
async fn send(address: SocketAddr, head: &str) -> String {
    let mut socket = TcpStream::connect(address).await.unwrap();
    let request = format!("{head}Connection: close\r\n\r\n");
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
    // Boxed because its size depends on how the build unifies features: over 16 KiB with the whole workspace, under it
    // alone, so an expectation of `clippy::large_futures` could hold in only one of the two.
    LocalSet::new()
        .run_until(Box::pin(async {
            let address = served(&MODULES).await;

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

/// The body of `response`, after its head.
fn body(response: &str) -> &str {
    response.split_once("\r\n\r\n").unwrap().1
}

#[tokio::test]
async fn it_serves_the_embedded_modules() {
    LocalSet::new()
        .run_until(Box::pin(async {
            let address = served(&MODULES).await;

            let list = get(address, "/modules/").await;
            assert!(list.starts_with("HTTP/1.1 200"));
            let names: Vec<String> = serde_json::from_str(body(&list)).unwrap();
            assert_eq!(names, ["@scope/math", "big", "page"]);
            assert!(get(address, "/modules").await.starts_with("HTTP/1.1 200"));

            let page = get(address, "/").await;
            assert!(page.contains("Content-Type: text/html"));
            assert_eq!(body(&page), "<p>page</p>");
            assert_eq!(body(&get(address, "/app.js").await), "app");
            assert_eq!(body(&get(address, "/modules/page/app.js").await), "app");
            assert!(get(address, "/modules/page/").await.starts_with("HTTP/1.1 404"));

            let wasm = get(address, "/modules/@scope/math/pkg/math.wasm").await;
            assert!(wasm.contains("Content-Type: application/wasm"));
            assert!(wasm.contains("ETag: \"wasm\"\r\n"));
            assert_eq!(body(&wasm), "\0asm");
            let wasm_path = "GET /modules/@scope/math/pkg/math.wasm HTTP/1.1\r\nHost: hub\r\n";
            for tags in ["\"wasm\"", "\"other\", W/\"wasm\"", "*"] {
                let revalidated = send(address, &format!("{wasm_path}If-None-Match: {tags}\r\n")).await;
                assert!(revalidated.starts_with("HTTP/1.1 304"));
            }
            let changed = send(address, &format!("{wasm_path}If-None-Match: \"other\", wasm\r\n")).await;
            assert_eq!(body(&changed), "\0asm");
            let encoded = get(address, "/modules/%40scope/math/package.json").await;
            assert_eq!(body(&encoded), "{}");
            assert!(
                get(address, "/modules/@scope/math/missing.js")
                    .await
                    .starts_with("HTTP/1.1 404")
            );
            assert!(
                get(address, "/modules/@scope/mathematics/x")
                    .await
                    .starts_with("HTTP/1.1 404")
            );
            assert!(get(address, "/modulesx").await.starts_with("HTTP/1.1 404"));

            let redirect = get(address, "/modules/big/dist/big.js").await;
            assert!(redirect.starts_with("HTTP/1.1 307"));
            assert!(redirect.contains("Location: https://cdn.example/npm/big/dist/big.js\r\n"));
            assert!(get(address, "/modules/bigger/x.js").await.starts_with("HTTP/1.1 404"));

            assert!(request(address, "POST", "/app.js").await.starts_with("HTTP/1.1 405"));
            assert!(request(address, "POST", "/missing").await.starts_with("HTTP/1.1 404"));
        }))
        .await;
}

#[tokio::test]
async fn modules_assembled_at_run_time_serve_as_built_in_ones_do() {
    // Read from somewhere only known once running, rather than built in, and so leaked to live as long as the server.
    let file = ModuleFile::new("text/plain", Box::leak(b"run".to_vec().into_boxed_slice()), "run");
    let files: &'static [(&'static str, ModuleFile)] = Box::leak(Box::new([("a.txt", file)]));
    let module = Module::new("assembled", files);
    // No page at `/`, and a module named as served elsewhere with nowhere to send it.
    let modules: &'static Modules = Box::leak(Box::new(Modules::new(Box::leak(Box::new([module])), "", &["far"], "")));
    LocalSet::new()
        .run_until(Box::pin(async {
            let address = served(modules).await;
            assert_eq!(body(&get(address, "/modules/assembled/a.txt").await), "run");
            assert!(get(address, "/").await.starts_with("HTTP/1.1 404"));
            assert!(get(address, "/modules/far/x.js").await.starts_with("HTTP/1.1 404"));
            let names: Vec<String> = serde_json::from_str(body(&get(address, "/modules/").await)).unwrap();
            assert_eq!(names, ["assembled", "far"]);
        }))
        .await;
}
