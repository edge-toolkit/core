//! Explores the host build of the firmware the way a page or a runner would: every module it lists, fetched.
//!
//! It asks the server what it serves rather than being told, so it holds for whatever modules the build put in: none
//! in a plain test run, and a whole scenario's when that scenario's `firmware-host-test` task builds it. A module built
//! in has to serve its `package.json` and its `main`, and answer a revalidation without the body; one served elsewhere
//! has to redirect to the same path under its base, which is not fetched, since it is not this server's to serve.
#![cfg(test)]

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{SocketAddr, TcpStream};
use std::process::{Child, Stdio};

use edge_toolkit::ws::{ClientMessage, ServerMessage};
use tungstenite::Message;

/// The host binary, serving on a port of its choosing, killed when dropped.
struct Server {
    child: Child,
    address: SocketAddr,
}

impl Server {
    /// The binary, started on a free loopback port, once it has said which.
    fn start() -> Self {
        // The raw child, since the server is killed rather than left to exit with a status to check.
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_et-ws-server-pico2w"))
            .arg("127.0.0.1:0")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::default();
        let _read = BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let address = line
            .trim()
            .strip_prefix("Serving http://")
            .unwrap()
            .trim_end_matches('/');
        Self {
            address: address.parse().unwrap(),
            child,
        }
    }

    /// The response to a `GET path` carrying `headers`, each a whole `Name: value\r\n` line.
    fn get(&self, path: &str, headers: &str) -> Response {
        let mut socket = TcpStream::connect(self.address).unwrap();
        let request = format!("GET {path} HTTP/1.1\r\nHost: board\r\n{headers}Connection: close\r\n\r\n");
        socket.write_all(request.as_bytes()).unwrap();
        let mut raw = Vec::new();
        let _read = socket.read_to_end(&mut raw).unwrap();
        let split = raw.windows(4).position(|window| window == b"\r\n\r\n").unwrap();
        let (head, body) = raw.split_at(split);
        let head = String::from_utf8(head.to_vec()).unwrap();
        let status = head.split(' ').nth(1).unwrap().parse().unwrap();
        Response {
            body: body.strip_prefix(b"\r\n\r\n").unwrap().to_vec(),
            head,
            status,
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _killed = self.child.kill();
        let _waited = self.child.wait();
    }
}

/// One HTTP response, its head kept as text.
struct Response {
    body: Vec<u8>,
    head: String,
    status: u16,
}

impl Response {
    /// The value of the header `name`, or empty when the response has none.
    fn header(&self, name: &str) -> &str {
        let prefix = format!("{name}: ");
        self.head
            .lines()
            .find_map(|line| line.strip_prefix(prefix.as_str()))
            .unwrap_or_default()
    }
}

/// Fetch what a page loads of the module `name`, whose `package.json` came back as `package`.
#[expect(
    clippy::single_call_fn,
    reason = "the built-in branch of the exploration, kept apart from the walk"
)]
fn explore_embedded(server: &Server, name: &str, package: &Response) {
    let manifest: serde_json::Value = serde_json::from_slice(&package.body).unwrap();
    if let Some(main) = manifest.get("main").and_then(serde_json::Value::as_str) {
        let main = main.trim_start_matches(['.', '/']);
        assert_eq!(server.get(&format!("/modules/{name}/{main}"), "").status, 200);
    }
    let revalidation = format!("If-None-Match: {}\r\n", package.header("ETag"));
    let revalidated = server.get(&format!("/modules/{name}/package.json"), &revalidation);
    assert_eq!(revalidated.status, 304);
}

#[test]
fn it_serves_every_module_it_lists() {
    let server = Server::start();
    assert_eq!(server.get("/health", "").status, 200);

    let list = server.get("/modules/", "");
    assert_eq!(list.status, 200);
    let names: Vec<String> = serde_json::from_slice(&list.body).unwrap();
    let mut built_in = 0_usize;
    for name in &names {
        let package = server.get(&format!("/modules/{name}/package.json"), "");
        if package.status == 307 {
            let location = package.header("Location");
            assert!(location.starts_with("https://"));
            assert!(location.ends_with(&format!("/{name}/package.json")));
        } else {
            assert_eq!(package.status, 200);
            explore_embedded(&server, name, &package);
            built_in = built_in.saturating_add(1);
        }
    }

    // A deployment that builds modules in always names its page among them, and one that builds none serves no page.
    let page = server.get("/", "");
    if built_in == 0 {
        assert_eq!(page.status, 404);
    } else {
        assert_eq!(page.status, 200);
        assert!(page.header("Content-Type").starts_with("text/html"));
    }
}

/// Send `message` to the hub over `socket`, and decode its reply.
fn exchange(
    socket: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>,
    message: &ClientMessage,
) -> ServerMessage {
    socket
        .send(Message::text(serde_json::to_string(message).unwrap()))
        .unwrap();
    ServerMessage::from_text_frame(socket.read().unwrap().to_text().unwrap()).unwrap()
}

#[test]
fn an_agent_is_given_an_id_and_told_the_time() {
    // The host's own ids and clock, which the board's hub stamps every agent and message with in its place.
    let server = Server::start();
    let (mut socket, _response) = tungstenite::connect(format!("ws://{}/ws", server.address)).unwrap();

    let connected = exchange(&mut socket, &ClientMessage::Connect { agent_id: None });
    assert!(matches!(connected, ServerMessage::ConnectAck { agent_id, .. } if agent_id == "host-1"));
    let alive = ClientMessage::Alive {
        timestamp: "now".to_owned(),
    };
    let ServerMessage::Response { message } = exchange(&mut socket, &alive) else {
        panic!("the alive message was not answered");
    };
    let (_said, time) = message.rsplit_once(" at ").unwrap();
    let _parsed = chrono::DateTime::parse_from_rfc3339(time).unwrap();
}
