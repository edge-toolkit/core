//! Blocking an address or evicting an agent through one hub of a NATS cluster cuts the address off from every hub:
//! open sockets close, upgrades and storage requests are refused, and a hub that joins later refuses it too. On a
//! single hub, the next frame from a blocked address closes its socket.
//!
//! Every hub and client here is on 127.0.0.1, so the block takes in the test's own connections as well; that is what
//! makes the refusals observable.
#![cfg(test)]

use std::time::Duration;

use et_test_nats::NatsServer;
use et_ws_test_server::{ADMIN_TOKEN, TestServer, connect_agent, start_clustered};
use futures_util::{SinkExt as _, StreamExt as _};
use reqwest::{Method, Response, StatusCode};
use tempfile::TempDir;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::http::StatusCode as UpgradeStatus;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::{Bytes, Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// How long a block may take to reach another hub and its next heartbeat.
const PROPAGATION: Duration = Duration::from_secs(10);

/// Send an admin `method` request for `path` to `hub`.
async fn admin(hub: &TestServer, method: Method, path: &str) -> Response {
    reqwest::Client::new()
        .request(method, format!("{}{path}", hub.base_url))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await
        .unwrap()
}

/// Assert the hub closes `socket` for policy, skipping whatever it sends first.
async fn assert_closed_for_policy(socket: &mut WebSocketStream<MaybeTlsStream<TcpStream>>) {
    let close = loop {
        match tokio::time::timeout(PROPAGATION, socket.next()).await.unwrap() {
            Some(Ok(Message::Close(frame))) => break frame.unwrap(),
            Some(Ok(_other)) => {}
            unexpected => panic!("expected a close frame, got {unexpected:?}"),
        }
    };
    assert_eq!(close.code, CloseCode::Policy);
}

/// Two hubs sharing one NATS server and storage directory, with an agent connected to the second.
struct Cluster {
    nats: NatsServer,
    storage: TempDir,
    first: TestServer,
    second: TestServer,
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    agent_id: String,
}

impl Cluster {
    async fn start() -> Self {
        let nats = NatsServer::start();
        let storage = TempDir::new().unwrap();
        let first = start_clustered(&nats.url(), storage.path());
        let second = start_clustered(&nats.url(), storage.path());
        let (socket, agent_id) = connect_agent(&second.ws_url).await;
        Self {
            nats,
            storage,
            first,
            second,
            socket,
            agent_id,
        }
    }

    /// Assert the agent's socket is closed for policy, and that both hubs and one joining now refuse 127.0.0.1.
    async fn assert_cut_off_everywhere(mut self) {
        assert_closed_for_policy(&mut self.socket).await;

        let late = start_clustered(&self.nats.url(), self.storage.path());
        for hub in [&self.first, &self.second, &late] {
            let upgrade = tokio_tungstenite::connect_async(&hub.ws_url).await;
            assert!(matches!(upgrade, Err(WsError::Http(response)) if response.status() == UpgradeStatus::FORBIDDEN));
            let stored = reqwest::get(format!("{}/storage/{}/file.txt", hub.base_url, self.agent_id))
                .await
                .unwrap();
            assert_eq!(stored.status(), StatusCode::FORBIDDEN);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_eviction_on_one_hub_cuts_the_address_off_everywhere() {
    let cluster = Cluster::start().await;
    let agent_id = cluster.agent_id.clone();

    let body = admin(&cluster.first, Method::DELETE, &format!("/agents/{agent_id}"))
        .await
        .error_for_status()
        .unwrap()
        .text()
        .await
        .unwrap();
    let evicted: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        evicted,
        serde_json::json!({"agent_id": agent_id, "blocked_ip": "127.0.0.1"})
    );

    cluster.assert_cut_off_everywhere().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocking_an_address_on_one_hub_cuts_it_off_everywhere() {
    let cluster = Cluster::start().await;

    let response = admin(&cluster.first, Method::PUT, "/blocked-ips/127.0.0.1").await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    cluster.assert_cut_off_everywhere().await;
}

/// A frame from a blocked address closes its socket rather than being handled.
///
/// The heartbeat would close the socket within a second anyway, so this shows the frame is refused rather than which
/// of the two closed it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_frame_from_a_blocked_address_closes_its_socket() {
    let hub = et_ws_test_server::start();
    let (mut socket, _agent_id) = connect_agent(&hub.ws_url).await;

    let response = admin(&hub, Method::PUT, "/blocked-ips/127.0.0.1").await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    socket.send(Message::Ping(Bytes::new())).await.unwrap();

    assert_closed_for_policy(&mut socket).await;
}
