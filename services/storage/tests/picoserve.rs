//! Agent storage through picoserve over real TCP sockets, with tokio standing in for an embedded executor and network
//! stack: what an agent stores any client reads back, and only the owner, connected, may store.
#![cfg(test)]

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use edge_toolkit::ws_server::AgentRegistry;
use et_storage_service::picoserve::{MAX_STORED, NotStored, Storage, StorageRoutes, path};
use picoserve::{Config, Router, Server};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::LocalSet;

/// How long a test waits for an answer before failing rather than hanging.
const PATIENCE: Duration = Duration::from_secs(5);
/// The agent the tests store files for, connected from this host.
const AGENT: &str = "id-1";

/// Agents' storage in memory, as a board keeps it.
#[derive(Default)]
struct Memory(Mutex<BTreeMap<(String, String), Vec<u8>>>);

impl Storage for Memory {
    fn get(&self, agent_id: &str, filename: &str) -> Result<Vec<u8>, NotStored> {
        let key = (agent_id.to_owned(), filename.to_owned());
        let Ok(stored) = self.0.lock() else {
            return Err(NotStored);
        };
        stored.get(&key).cloned().ok_or(NotStored)
    }

    fn set(&self, agent_id: &str, filename: &str, contents: Vec<u8>) {
        let _previous = self
            .0
            .lock()
            .unwrap()
            .insert((agent_id.to_owned(), filename.to_owned()), contents);
    }
}

/// Storage on a fresh loopback port, one task per connection as an embedder would, and its address.
///
/// Also the registry its owners are checked against, [`AGENT`] connected in it.
async fn serving() -> (SocketAddr, Arc<AgentRegistry<()>>) {
    let registry = Arc::new(AgentRegistry::default());
    let localhost = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let _connected = registry.connect_agent(Some(AGENT.to_owned()), AGENT.to_owned(), localhost, ());
    let storage = Arc::new(Memory::default());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let accepting = Arc::clone(&registry);
    let _accepting = tokio::task::spawn_local(async move {
        loop {
            let (socket, peer) = listener.accept().await.unwrap();
            let registry = Arc::clone(&accepting);
            let storage = Arc::clone(&storage);
            let _serving = tokio::task::spawn_local(async move {
                let routes = StorageRoutes::new(&*registry, &*storage, peer.ip());
                let app = Router::new().route_service(path(), routes);
                let config = Config::const_default();
                let mut http_buffer = [0; 2048];
                let _served = Server::new_tokio(&app, &config, &mut http_buffer).serve(socket).await;
            });
        }
    });
    (address, registry)
}

/// The whole response to `head`, a request line and headers to which the closing ones are added, and then `body`.
async fn send(address: SocketAddr, head: &str, body: &[u8]) -> String {
    let mut socket = TcpStream::connect(address).await.unwrap();
    let request = format!("{head}Connection: close\r\n\r\n");
    socket.write_all(request.as_bytes()).await.unwrap();
    socket.write_all(body).await.unwrap();
    let mut response = String::default();
    let _read = tokio::time::timeout(PATIENCE, socket.read_to_string(&mut response))
        .await
        .unwrap()
        .unwrap();
    response
}

/// The whole response to a bodiless `method path`.
async fn request(address: SocketAddr, method: &str, path: &str) -> String {
    send(address, &format!("{method} {path} HTTP/1.1\r\nHost: hub\r\n"), b"").await
}

/// The whole response to a `PUT path` of `contents`.
async fn put(address: SocketAddr, path: &str, contents: &[u8]) -> String {
    let head = format!(
        "PUT {path} HTTP/1.1\r\nHost: hub\r\nContent-Length: {}\r\n",
        contents.len()
    );
    send(address, &head, contents).await
}

/// The body of `response`, after its head.
fn body(response: &str) -> &str {
    response.split_once("\r\n\r\n").unwrap().1
}

#[tokio::test]
async fn an_agent_stores_files_any_client_reads_back() {
    LocalSet::new()
        .run_until(Box::pin(async {
            let (address, registry) = serving().await;
            let path = format!("/storage/{AGENT}/model.json");

            assert!(request(address, "GET", &path).await.starts_with("HTTP/1.1 404"));
            assert!(put(address, &path, b"{\"w\":1}").await.starts_with("HTTP/1.1 200"));
            assert_eq!(body(&request(address, "GET", &path).await), "{\"w\":1}");
            let head = request(address, "HEAD", &path).await;
            assert!(head.starts_with("HTTP/1.1 200"));
            assert_eq!(body(&head), "");
            // Replaced by the next upload, and decoded as the `std` service decodes it.
            assert!(put(address, &path, b"2").await.starts_with("HTTP/1.1 200"));
            let encoded = request(address, "GET", &format!("/storage/{AGENT}/model%2Ejson")).await;
            assert_eq!(body(&encoded), "2");

            let too_large = vec![0; MAX_STORED + 1];
            let refused = put(address, &format!("/storage/{AGENT}/big"), &too_large).await;
            assert!(refused.starts_with("HTTP/1.1 413"));
            assert!(
                put(address, "/storage/nobody/x", b"1")
                    .await
                    .starts_with("HTTP/1.1 404")
            );
            assert!(request(address, "DELETE", &path).await.starts_with("HTTP/1.1 405"));

            // Only while the agent is connected.
            registry.mark_disconnected(AGENT);
            assert!(put(address, &path, b"3").await.starts_with("HTTP/1.1 403"));
            assert_eq!(body(&request(address, "GET", &path).await), "2");

            let localhost = IpAddr::V4(Ipv4Addr::LOCALHOST);
            let _again = registry.connect_agent(Some(AGENT.to_owned()), AGENT.to_owned(), localhost, ());
            let _blocked = registry.blocklist.block(localhost);
            assert!(put(address, &path, b"4").await.starts_with("HTTP/1.1 403"));
        }))
        .await;
}

#[tokio::test]
async fn a_path_naming_no_stored_file_is_not_found() {
    LocalSet::new()
        .run_until(Box::pin(async {
            let (address, _registry) = serving().await;
            for path in [
                "/storage/id-1",
                "/storage/id-1/a/b",
                "/storage/id-1/%2F",
                "/storage/id-1/..",
                "/storage//a",
                "/storage/id-1/%zz",
            ] {
                assert!(request(address, "GET", path).await.starts_with("HTTP/1.1 404"));
            }
        }))
        .await;
}
