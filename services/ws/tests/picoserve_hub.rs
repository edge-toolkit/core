//! The picoserve hub over real TCP sockets, with tokio standing in for an embedded executor and network stack: agents
//! connect over HTTP, upgrade to WebSocket, and reach each other through the one hub.
#![cfg(test)]
#![recursion_limit = "256"]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use edge_toolkit::ws::{
    AgentConnectionState, ClientMessage, ConnectStatus, MessageDeliveryStatus, MessageScope, ServerMessage,
};
use edge_toolkit::ws_server::MessageRetention;
use et_ws_service::hub::{SessionMessage, Stamps};
use et_ws_service::picoserve_hub::{Hub, LocalLink, Outbox, config};
use futures_util::{SinkExt as _, StreamExt as _};
use picoserve::time::TokioTimer;
use serde_json::json;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::LocalSet;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

/// The largest frame the hub under test reads.
const MAX_FRAME_SIZE: usize = 1024;
/// How many messages the hub under test holds for a connection.
const MAX_QUEUED: usize = 16;
/// How long a test waits for a frame before failing rather than hanging.
const PATIENCE: Duration = Duration::from_secs(5);

type Client = WebSocketStream<MaybeTlsStream<TcpStream>>;

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

/// Run the test `body` makes on one thread, as an embedded executor runs the hub's connections.
#[expect(
    clippy::future_not_send,
    reason = "a LocalSet is one thread by design, as picoserve's connections need"
)]
async fn on_one_thread<B, F>(body: B)
where
    B: FnOnce() -> F,
    F: Future<Output = ()>,
{
    LocalSet::new().run_until(Box::pin(body())).await;
}

/// A hub served on a fresh loopback port, one task per connection as an embedder would, and its address.
async fn served_hub() -> (Arc<Hub<Counter>>, SocketAddr) {
    let hub = Arc::new(Hub::new(
        Counter::default(),
        MessageRetention::All,
        MAX_FRAME_SIZE,
        MAX_QUEUED,
    ));
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let serving = Arc::clone(&hub);
    let _accepting = tokio::task::spawn_local(async move {
        loop {
            let (socket, peer) = listener.accept().await.unwrap();
            let hub = Arc::clone(&serving);
            let _serving = tokio::task::spawn_local(async move {
                let mut http_buffer = [0; 2048];
                let _served = hub
                    .serve(socket, peer.ip(), TokioTimer, &config(), &mut http_buffer)
                    .await;
            });
        }
    });
    (hub, address)
}

/// A WebSocket open on `/ws` that has not connected as an agent.
async fn open(address: SocketAddr) -> Client {
    connect_async(format!("ws://{address}/ws")).await.unwrap().0
}

/// A client connected as `agent_id`, with the status its connect was acknowledged with.
async fn connect(address: SocketAddr, agent_id: &str) -> (Client, ConnectStatus) {
    let mut client = open(address).await;
    let connect = ClientMessage::Connect {
        agent_id: Some(agent_id.to_owned()),
    };
    send(&mut client, &connect).await;
    let ServerMessage::ConnectAck { status, .. } = receive(&mut client).await else {
        panic!("the connect was not acknowledged");
    };
    (client, status)
}

async fn send(client: &mut Client, message: &ClientMessage) {
    client
        .send(Message::text(serde_json::to_string(message).unwrap()))
        .await
        .unwrap();
}

/// The next frame `client` is sent.
async fn next_frame(client: &mut Client) -> Message {
    tokio::time::timeout(PATIENCE, client.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}

/// The next frame `client` is sent, as a protocol message.
async fn receive(client: &mut Client) -> ServerMessage {
    ServerMessage::from_text_frame(next_frame(client).await.to_text().unwrap()).unwrap()
}

/// Whether `message`, sent by `client` directly to the agent `to_agent_id`, was answered as delivered.
async fn delivered(client: &mut Client, to_agent_id: &str, message: serde_json::Value) -> bool {
    sent_directly(client, to_agent_id, message, &MessageDeliveryStatus::Delivered).await
}

/// Whether `message`, sent by `client` directly to the agent `to_agent_id`, was answered with `expected`.
async fn sent_directly(
    client: &mut Client,
    to_agent_id: &str,
    message: serde_json::Value,
    expected: &MessageDeliveryStatus,
) -> bool {
    let direct = ClientMessage::SendAgentMessage {
        to_agent_id: to_agent_id.to_owned(),
        message,
    };
    send(client, &direct).await;
    matches!(receive(client).await, ServerMessage::MessageStatus { status, .. } if status == *expected)
}

#[tokio::test]
async fn agents_on_separate_sockets_reach_each_other() {
    on_one_thread(async || {
        let (_hub, address) = served_hub().await;
        let (mut alice, alice_status) = connect(address, "alice").await;
        let (mut bob, _bob_status) = connect(address, "bob").await;
        assert_eq!(alice_status, ConnectStatus::Assigned);

        assert!(delivered(&mut alice, "bob", json!({"hello": "bob"})).await);
        assert!(matches!(
            receive(&mut bob).await,
            ServerMessage::AgentMessage { from_agent_id, scope: MessageScope::Direct, .. } if from_agent_id == "alice"
        ));

        send(&mut bob, &ClientMessage::ListAgents).await;
        assert!(matches!(
            receive(&mut bob).await,
            ServerMessage::ListAgentsResponse { agents } if agents.len() == 2
        ));

        // Frames outside the protocol are relayed to every other agent as they came.
        bob.send(Message::text("raw text")).await.unwrap();
        assert_eq!(next_frame(&mut alice).await, Message::text("raw text"));
        bob.send(Message::binary(vec![1, 2, 3])).await.unwrap();
        assert_eq!(next_frame(&mut alice).await, Message::binary(vec![1, 2, 3]));
    })
    .await;
}

#[tokio::test]
async fn a_message_waits_for_a_recipient_that_has_gone() {
    on_one_thread(async || {
        let (_hub, address) = served_hub().await;
        let (mut alice, _alice_status) = connect(address, "alice").await;
        let (mut bob, _bob_status) = connect(address, "bob").await;
        bob.close(None).await.unwrap();
        // The close is answered once the hub has released bob's id.
        while bob.next().await.is_some() {}

        assert!(sent_directly(&mut alice, "bob", json!(null), &MessageDeliveryStatus::Queued).await);

        let (mut bob, status) = connect(address, "bob").await;
        assert_eq!(status, ConnectStatus::Reconnected);
        assert!(matches!(
            receive(&mut bob).await,
            ServerMessage::AgentMessage { from_agent_id, .. } if from_agent_id == "alice"
        ));
    })
    .await;
}

#[tokio::test]
async fn control_frames_and_undecodable_frames_leave_the_connection_open() {
    on_one_thread(async || {
        let (_hub, address) = served_hub().await;
        let (mut alice, _alice_status) = connect(address, "alice").await;

        alice.send(Message::Ping(vec![7].into())).await.unwrap();
        assert_eq!(next_frame(&mut alice).await, Message::Pong(vec![7].into()));
        alice.send(Message::Pong(vec![8].into())).await.unwrap();
        alice
            .send(Message::text(r#"{"type":"et-connect","agent_id":5}"#))
            .await
            .unwrap();

        send(&mut alice, &ClientMessage::ListAgents).await;
        assert!(matches!(
            receive(&mut alice).await,
            ServerMessage::ListAgentsResponse { agents } if agents.len() == 1
        ));
    })
    .await;
}

#[tokio::test]
async fn a_frame_larger_than_the_hub_reads_ends_its_connection() {
    on_one_thread(async || {
        let (_hub, address) = served_hub().await;
        let mut client = open(address).await;
        client.send(Message::binary(vec![0; MAX_FRAME_SIZE + 1])).await.unwrap();
        let ended = tokio::time::timeout(PATIENCE, client.next()).await.unwrap();
        assert!(!matches!(ended, Some(Ok(Message::Text(_) | Message::Binary(_)))));
    })
    .await;
}

#[tokio::test]
async fn a_blocked_address_is_refused_and_other_paths_are_not_found() {
    on_one_thread(async || {
        let (hub, address) = served_hub().await;
        let missing = connect_async(format!("ws://{address}/elsewhere")).await.unwrap_err();
        assert!(matches!(missing, WsError::Http(response) if response.status() == 404));

        assert!(hub.registry().blocklist.block(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        let refused = connect_async(format!("ws://{address}/ws")).await.unwrap_err();
        assert!(matches!(refused, WsError::Http(response) if response.status() == 403));
    })
    .await;
}

/// The next message `link`'s agent is sent, as a protocol message.
async fn local_receive(link: &mut LocalLink<'_, Counter>) -> ServerMessage {
    let SessionMessage::Json(message) = tokio::time::timeout(PATIENCE, link.next()).await.unwrap() else {
        panic!("a raw frame arrived where a message was expected");
    };
    message
}

#[tokio::test]
async fn an_agent_in_process_reaches_one_on_a_socket_and_leaves_when_dropped() {
    on_one_thread(async || {
        let (hub, address) = served_hub().await;
        let (mut alice, _alice_status) = connect(address, "alice").await;
        let mut board = hub.local(IpAddr::V4(Ipv4Addr::LOCALHOST));
        board.send(ClientMessage::Connect {
            agent_id: Some("board".to_owned()),
        });
        assert!(matches!(
            local_receive(&mut board).await,
            ServerMessage::ConnectAck { agent_id, status: ConnectStatus::Assigned } if agent_id == "board"
        ));

        board.send(ClientMessage::SendAgentMessage {
            to_agent_id: "alice".to_owned(),
            message: json!({"from": "the board"}),
        });
        assert!(matches!(
            local_receive(&mut board).await,
            ServerMessage::MessageStatus {
                status: MessageDeliveryStatus::Delivered,
                ..
            }
        ));
        assert!(matches!(
            receive(&mut alice).await,
            ServerMessage::AgentMessage { from_agent_id, .. } if from_agent_id == "board"
        ));

        assert!(delivered(&mut alice, "board", json!({"from": "alice"})).await);
        assert!(matches!(
            local_receive(&mut board).await,
            ServerMessage::AgentMessage { from_agent_id, .. } if from_agent_id == "alice"
        ));

        board.send_binary(Bytes::from_static(&[4, 5, 6])).unwrap();
        assert_eq!(next_frame(&mut alice).await, Message::binary(vec![4, 5, 6]));

        drop(board);
        send(&mut alice, &ClientMessage::ListAgents).await;
        let ServerMessage::ListAgentsResponse { agents } = receive(&mut alice).await else {
            panic!("the agents were not listed");
        };
        let board_state = agents
            .iter()
            .find(|agent| agent.agent_id == "board")
            .map(|agent| &agent.state);
        assert_eq!(board_state, Some(&AgentConnectionState::Disconnected));
    })
    .await;
}

#[tokio::test]
async fn an_agent_in_process_is_held_to_the_frames_a_connection_may_send() {
    let hub = Hub::new(Counter::default(), MessageRetention::All, MAX_FRAME_SIZE, MAX_QUEUED);
    let mut board = hub.local(IpAddr::V4(Ipv4Addr::LOCALHOST));
    let oversized = MAX_FRAME_SIZE.saturating_add(1);
    let refused = board.send_binary(Bytes::from(vec![0; oversized])).unwrap_err();
    assert_eq!((refused.size, refused.max), (oversized, MAX_FRAME_SIZE));
    let refused = board.send_text(&" ".repeat(oversized)).unwrap_err();
    assert_eq!(
        refused.to_string(),
        "a 1025-byte frame is larger than the 1024 bytes the hub reads"
    );

    // Text that is no message is dropped, as a connection's is, so the first reply is the connect's.
    board.send_text("not a message").unwrap();
    let connect = serde_json::to_string(&ClientMessage::Connect { agent_id: None }).unwrap();
    board.send_text(&connect).unwrap();
    assert!(matches!(
        local_receive(&mut board).await,
        ServerMessage::ConnectAck { .. }
    ));
}

#[tokio::test]
async fn an_agent_in_process_that_never_reads_keeps_only_its_latest_replies() {
    let hub = Hub::new(Counter::default(), MessageRetention::All, MAX_FRAME_SIZE, MAX_QUEUED);
    let mut board = hub.local(IpAddr::V4(Ipv4Addr::LOCALHOST));
    board.send(ClientMessage::Connect { agent_id: None });
    for _ in 0..MAX_QUEUED.saturating_mul(3) {
        board.send(ClientMessage::Alive {
            timestamp: "now".to_owned(),
        });
    }
    board.send(ClientMessage::ListAgents);
    for _ in 1..MAX_QUEUED {
        assert!(matches!(
            local_receive(&mut board).await,
            ServerMessage::Response { .. }
        ));
    }
    assert!(matches!(
        local_receive(&mut board).await,
        ServerMessage::ListAgentsResponse { .. }
    ));
    let _nothing_more = tokio::time::timeout(Duration::from_millis(100), board.next())
        .await
        .unwrap_err();
}

#[tokio::test]
async fn an_agent_in_process_reconnecting_to_more_than_its_outbox_holds_is_still_acknowledged() {
    on_one_thread(async || {
        let (hub, address) = served_hub().await;
        let (mut alice, _alice_status) = connect(address, "alice").await;
        let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let connect = || ClientMessage::Connect {
            agent_id: Some("board".to_owned()),
        };
        let mut board = hub.local(loopback);
        board.send(connect());
        drop(board);
        // One more than the outbox holds, all waiting for the board while it is away.
        for _ in 0..=MAX_QUEUED {
            assert!(sent_directly(&mut alice, "board", json!(null), &MessageDeliveryStatus::Queued).await);
        }

        let mut board = hub.local(loopback);
        board.send(connect());
        assert!(matches!(
            local_receive(&mut board).await,
            ServerMessage::ConnectAck {
                status: ConnectStatus::Reconnected,
                ..
            }
        ));
        for _ in 0..=MAX_QUEUED {
            assert!(matches!(
                local_receive(&mut board).await,
                ServerMessage::AgentMessage { from_agent_id, .. } if from_agent_id == "alice"
            ));
        }
    })
    .await;
}

#[tokio::test]
async fn a_full_outbox_drops_its_oldest_message() {
    let outbox = Outbox::new(2);
    for text in ["first", "second", "third"] {
        outbox.send(SessionMessage::Text(text.to_owned()));
    }
    assert!(matches!(outbox.next().await, SessionMessage::Text(text) if text == "second"));
    assert!(matches!(outbox.next().await, SessionMessage::Text(text) if text == "third"));
}
