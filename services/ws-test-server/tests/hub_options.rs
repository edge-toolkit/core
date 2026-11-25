//! A hub started through `start_with` is set up as its `HubOptions` say: its workers, its listeners, the most sockets
//! a worker holds, and the routes a caller mounts beside the hub's own.
#![cfg(test)]

use std::sync::Arc;
use std::time::Duration;

use actix_web::{HttpResponse, web};
use edge_toolkit::ws::{ClientMessage, ServerMessage};
use et_ws_test_server::{HubOptions, Transport, connect_agent, start_with};
use futures_util::{SinkExt as _, StreamExt as _};
use pretty_assertions::assert_eq;
use pretty_assertions::assert_ne;
use rustls::pki_types::ServerName;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{client_async, connect_async};

/// What the route the caller mounts below answers.
const HOOKED: &str = "hooked";

#[test_log::test(tokio::test(flavor = "multi_thread", worker_threads = 2))]
async fn one_worker_serves_agents_on_both_loopback_addresses_and_the_callers_route() {
    let mut options = HubOptions::default();
    options.workers = 1;
    options.ipv6 = true;
    options.configure = Arc::new(|cfg| {
        let _routed = cfg.route("/hook", web::get().to(|| async { HttpResponse::Ok().body(HOOKED) }));
    });
    let server = start_with(options);

    assert_eq!(server.ws_urls.len(), 2);
    assert_eq!(server.ws_urls.first(), Some(&server.ws_url));
    assert!(server.ws_urls.iter().any(|url| url.starts_with("ws://[::1]:")));
    for url in &server.ws_urls {
        let (_socket, agent_id) = connect_agent(url).await;
        assert_ne!(agent_id, "");
    }
    let body = reqwest::get(format!("{}/hook", server.base_url))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(body, HOOKED);
}

#[test_log::test(tokio::test(flavor = "multi_thread", worker_threads = 2))]
async fn an_https_hub_registers_an_agent_that_trusts_its_self_signed_certificate() {
    let (transport, trusted) = Transport::self_signed();
    let mut options = HubOptions::default();
    options.transport = transport;
    let server = start_with(options);
    assert!(server.base_url.starts_with("https://127.0.0.1:"));

    let address = server.base_url.strip_prefix("https://").unwrap().to_owned();
    let tcp = TcpStream::connect(&address).await.unwrap();
    let ip = tcp.peer_addr().unwrap().ip();
    let tls = TlsConnector::from(trusted)
        .connect(ServerName::from(ip), tcp)
        .await
        .unwrap();
    let (mut socket, _response) = client_async(server.ws_url.as_str(), tls).await.unwrap();
    let connect = serde_json::to_string(&ClientMessage::Connect { agent_id: None }).unwrap();
    socket.send(Message::text(connect)).await.unwrap();
    let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
        panic!("expected the hub's et-connect-ack as a text frame");
    };
    assert!(matches!(
        ServerMessage::from_text_frame(&text),
        Ok(ServerMessage::ConnectAck { .. }),
    ));
}

#[test_log::test(tokio::test(flavor = "multi_thread", worker_threads = 2))]
async fn a_worker_at_its_most_connections_holds_back_the_next_until_one_closes() {
    let mut options = HubOptions::default();
    options.workers = 1;
    options.max_connections = 1;
    let server = start_with(options);

    let (mut first, _first_id) = connect_agent(&server.ws_url).await;
    let url = server.ws_url.clone();
    let mut second = tokio::spawn(async move { connect_async(url).await.map(|_connected| ()) });
    let _elapsed = tokio::time::timeout(Duration::from_secs(1), &mut second)
        .await
        .unwrap_err();

    first.send(Message::Close(None)).await.unwrap();
    drop(first);
    tokio::time::timeout(Duration::from_secs(10), second)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
