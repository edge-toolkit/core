//! Several hubs sharing one NATS backplane behave as one hub, whichever hub each agent is attached to.
//!
//! Every test starts its own broker and its own hubs, so the directory, the mailbox and the delivery fabric are all
//! crossed for real: a message from an agent on one hub to an agent on another goes out through NATS and back in.
#![cfg(test)]

use std::collections::BTreeSet;
use std::time::Duration;

use edge_toolkit::ws::{AgentConnectionState, AgentSummary, MessageDeliveryStatus, MessageScope, ServerMessage};
use et_test_nats::NatsServer;
use et_ws_test_server::{TestServer, connect_agent, connect_agent_as, next_payload, start_clustered};
use futures_util::{SinkExt as _, StreamExt as _};
use tempfile::TempDir;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::{Bytes, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// A client connection to a hub.
type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// One broker, the hubs sharing it, and the storage root they all write to.
struct Cluster {
    hubs: Vec<TestServer>,
    // Kept for their drop: the storage root and the broker outlive the hubs using them.
    _storage: TempDir,
    nats: NatsServer,
}

fn cluster(size: usize) -> Cluster {
    let nats = NatsServer::start();
    let storage = TempDir::new().unwrap();
    let hubs = std::iter::repeat_with(|| start_clustered(&nats.url(), storage.path()))
        .take(size)
        .collect();
    Cluster {
        hubs,
        _storage: storage,
        nats,
    }
}

/// One agent on each hub of `cluster`, in hub order, with the id each was assigned.
async fn one_agent_per_hub(cluster: &Cluster) -> Vec<(Socket, String)> {
    let mut agents = Vec::with_capacity(cluster.hubs.len());
    for hub in &cluster.hubs {
        agents.push(connect_agent(&hub.ws_url).await);
    }
    agents
}

async fn send_json(socket: &mut Socket, body: &serde_json::Value) {
    socket.send(Message::text(body.to_string())).await.unwrap();
}

async fn send_direct(socket: &mut Socket, to_agent_id: &str, message: serde_json::Value) {
    let request = serde_json::json!({"type": "et-send-agent-message", "to_agent_id": to_agent_id, "message": message});
    send_json(socket, &request).await;
}

async fn acknowledge(socket: &mut Socket, message_id: &str) {
    send_json(
        socket,
        &serde_json::json!({"type": "et-message-ack", "message_id": message_id}),
    )
    .await;
}

/// The next protocol message the hub sends on `socket`, skipping control frames.
async fn next_message(socket: &mut Socket) -> ServerMessage {
    loop {
        // Generous: a lost-broker reply waits out the NATS request timeout first, and the suite runs hubs in parallel.
        let frame = tokio::time::timeout(Duration::from_secs(30), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let Message::Text(text) = frame {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

/// The next delivery status on `socket`, with the message id it is about.
async fn next_status(socket: &mut Socket) -> (MessageDeliveryStatus, String) {
    loop {
        if let ServerMessage::MessageStatus {
            status,
            message_id: Some(message_id),
            ..
        } = next_message(socket).await
        {
            return (status, message_id);
        }
    }
}

/// The next agent message on `socket`: its id, sender, scope and body.
async fn next_agent_message(socket: &mut Socket) -> (String, String, MessageScope, serde_json::Value) {
    loop {
        if let ServerMessage::AgentMessage {
            message_id,
            from_agent_id,
            scope,
            message,
            ..
        } = next_message(socket).await
        {
            return (message_id, from_agent_id, scope, message);
        }
    }
}

/// Check `recipient` receives message `sent_id` and acknowledges it, and that both ends hear of the acknowledgement.
///
/// The message must come from `sender_id` and carry `body`.
async fn receive_and_acknowledge(
    recipient: &mut Socket,
    sender: &mut Socket,
    (sender_id, sent_id): (&str, &str),
    body: &serde_json::Value,
) {
    let (received_id, from, scope, received) = next_agent_message(recipient).await;
    assert_eq!((received_id.as_str(), from.as_str()), (sent_id, sender_id));
    assert_eq!((scope, &received), (MessageScope::Direct, body));

    acknowledge(recipient, sent_id).await;
    let acknowledged = (MessageDeliveryStatus::Acknowledged, sent_id.to_owned());
    assert_eq!(next_status(recipient).await, acknowledged);
    assert_eq!(next_status(sender).await, acknowledged);
}

/// Send `body` from `sender` to `to_agent_id`, check the hub answers `expected`, and return the message's id.
async fn send_and_expect(
    sender: &mut Socket,
    to_agent_id: &str,
    body: &serde_json::Value,
    expected: MessageDeliveryStatus,
) -> String {
    send_direct(sender, to_agent_id, body.clone()).await;
    let (status, sent_id) = next_status(sender).await;
    assert_eq!(status, expected);
    sent_id
}

/// Send `body` from `sender` to `to_agent_id`, and check it is reported delivered and reaches `recipient`.
async fn deliver(sender: &mut Socket, recipient: &mut Socket, to_agent_id: &str, body: &serde_json::Value) {
    let _sent_id = send_and_expect(sender, to_agent_id, body, MessageDeliveryStatus::Delivered).await;
    assert_eq!(next_agent_message(recipient).await.3, *body);
}

/// The detail of the invalid reply the hub behind `socket` sends next.
async fn next_invalid(socket: &mut Socket) -> String {
    let ServerMessage::Invalid { detail, .. } = next_message(socket).await else {
        panic!("expected an invalid reply");
    };
    detail
}

/// Check the hub behind `socket` answers its next request by reporting the backplane unavailable.
async fn expect_backplane_unavailable(socket: &mut Socket) {
    assert!(next_invalid(socket).await.starts_with("hub backplane unavailable"));
}

/// The roster the hub behind `socket` reports.
async fn roster(socket: &mut Socket) -> Vec<AgentSummary> {
    send_json(socket, &serde_json::json!({"type": "et-list-agents"})).await;
    loop {
        if let ServerMessage::ListAgentsResponse { agents } = next_message(socket).await {
            return agents;
        }
    }
}

/// Poll the roster behind `socket` until it lists `agent_id` in `state`.
async fn wait_for_state(socket: &mut Socket, agent_id: &str, state: &AgentConnectionState) {
    for _ in 0_u32..100 {
        if roster(socket)
            .await
            .iter()
            .any(|agent| agent.agent_id == agent_id && agent.state == *state)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{agent_id} never reached {state:?}");
}

/// Read `socket` until it has seen both its own Delivered status and the message `sender` sent it, in either order.
///
/// Returns the id of the message this socket sent and the id of the one it received.
#[expect(
    clippy::single_call_fn,
    reason = "the per-socket read step of the load test, kept out of its body"
)]
async fn delivered_and_received(socket: &mut Socket, sender: &str) -> (String, String) {
    let (mut sent, mut received) = (None, None);
    while sent.is_none() || received.is_none() {
        let message = next_message(socket).await;
        if let ServerMessage::MessageStatus {
            status: MessageDeliveryStatus::Delivered,
            message_id,
            ..
        } = &message
        {
            sent.clone_from(message_id);
        } else if let ServerMessage::AgentMessage {
            message_id,
            from_agent_id,
            message: body,
            ..
        } = message
        {
            assert_eq!(from_agent_id, sender);
            assert_eq!(body, serde_json::json!({"from": sender}));
            received = Some(message_id);
        } else {
            panic!("unexpected {message:?}");
        }
    }
    (sent.unwrap(), received.unwrap())
}

/// The two message ids `socket` next hears acknowledged: its own ack confirmed, and the receipt for what it sent.
#[expect(
    clippy::single_call_fn,
    reason = "the per-socket ack step of the load test, kept out of its body"
)]
async fn two_acknowledged(socket: &mut Socket) -> BTreeSet<String> {
    let mut acknowledged = BTreeSet::new();
    while acknowledged.len() < 2 {
        let (status, message_id) = next_status(socket).await;
        assert_eq!(status, MessageDeliveryStatus::Acknowledged);
        let _new = acknowledged.insert(message_id);
    }
    acknowledged
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_hub_lists_the_agents_of_every_hub() {
    let cluster = cluster(2);
    let mut agents = one_agent_per_hub(&cluster).await;
    let expected = agents.iter().map(|(_, id)| id.clone()).collect::<BTreeSet<_>>();

    for (socket, _) in &mut agents {
        let listed = roster(socket).await;
        let ids = listed
            .iter()
            .map(|agent| agent.agent_id.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(ids, expected);
        assert!(
            listed
                .iter()
                .all(|agent| agent.state == AgentConnectionState::Connected)
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_direct_message_and_its_ack_cross_between_hubs() {
    let cluster = cluster(2);
    let [(mut alice, alice_id), (mut bob, bob_id)] = <[_; 2]>::try_from(one_agent_per_hub(&cluster).await).unwrap();
    let body = serde_json::json!({"ping": 1_i32});

    let sent_id = send_and_expect(&mut alice, &bob_id, &body, MessageDeliveryStatus::Delivered).await;
    receive_and_acknowledge(&mut bob, &mut alice, (&alice_id, &sent_id), &body).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn broadcasts_and_relayed_frames_reach_every_hub_but_the_sender() {
    let cluster = cluster(3);
    let [(mut alice, alice_id), (mut bob, _), (mut carol, _)] =
        <[_; 3]>::try_from(one_agent_per_hub(&cluster).await).unwrap();
    // A neighbour on the sender's own hub hears it through that hub alone, not back over the backplane.
    let (mut dave, _) = connect_agent(&cluster.hubs[0].ws_url).await;

    let broadcast = serde_json::json!({"type": "et-broadcast-message", "message": {"hello": "all"}});
    send_json(&mut alice, &broadcast).await;
    assert_eq!(next_status(&mut alice).await.0, MessageDeliveryStatus::Broadcast);
    for socket in [&mut bob, &mut carol, &mut dave] {
        let (_, from, scope, body) = next_agent_message(socket).await;
        assert_eq!((from.as_str(), scope), (alice_id.as_str(), MessageScope::Broadcast));
        assert_eq!(body, serde_json::json!({"hello": "all"}));
    }

    let binary = Message::Binary(Bytes::from_static(&[7, 8, 9]));
    alice.send(Message::text("not an et frame")).await.unwrap();
    alice.send(binary.clone()).await.unwrap();
    for socket in [&mut bob, &mut carol, &mut dave] {
        assert_eq!(next_payload(socket).await, Message::text("not an et frame"));
        assert_eq!(next_payload(socket).await, binary);
    }
    // Nothing comes back to the sender: half a second of silence.
    let _elapsed = tokio::time::timeout(Duration::from_millis(500), alice.next())
        .await
        .unwrap_err();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_message_queued_via_one_hub_is_delivered_through_another() {
    let cluster = cluster(3);
    let (bob, _) = connect_agent_as(&cluster.hubs[0].ws_url, Some("agent-b")).await;
    let (mut alice, alice_id) = connect_agent(&cluster.hubs[1].ws_url).await;
    drop(bob);
    wait_for_state(&mut alice, "agent-b", &AgentConnectionState::Disconnected).await;

    let body = serde_json::json!({"while": "away"});
    let sent_id = send_and_expect(&mut alice, "agent-b", &body, MessageDeliveryStatus::Queued).await;

    let (mut bob, _) = connect_agent_as(&cluster.hubs[2].ws_url, Some("agent-b")).await;
    receive_and_acknowledge(&mut bob, &mut alice, (&alice_id, &sent_id), &body).await;
    // The hub it came back through now holds it, as every hub's roster says.
    wait_for_state(&mut alice, "agent-b", &AgentConnectionState::Connected).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_that_moves_hub_keeps_its_identity_and_its_messages() {
    let cluster = cluster(3);
    let (old, _) = connect_agent_as(&cluster.hubs[0].ws_url, Some("agent-t")).await;
    let (mut moved, _) = connect_agent_as(&cluster.hubs[1].ws_url, Some("agent-t")).await;
    let (mut alice, _) = connect_agent(&cluster.hubs[2].ws_url).await;

    deliver(&mut alice, &mut moved, "agent-t", &serde_json::json!({"n": 1_i32})).await;

    // The socket left behind on the first hub closes late; the agent must stay where it moved to.
    drop(old);
    tokio::time::sleep(Duration::from_millis(500)).await;
    wait_for_state(&mut alice, "agent-t", &AgentConnectionState::Connected).await;
    deliver(&mut alice, &mut moved, "agent-t", &serde_json::json!({"n": 2_i32})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn storage_accepts_an_upload_through_a_hub_the_agent_never_joined() {
    let cluster = cluster(2);
    let (_alice, alice_id) = connect_agent(&cluster.hubs[0].ws_url).await;
    let client = reqwest::Client::new();

    let put_url = format!("{}/storage/{alice_id}/result.txt", cluster.hubs[1].base_url);
    let put = client.put(put_url).body("42").send().await.unwrap();
    assert!(put.status().is_success());

    let get_url = format!("{}/storage/{alice_id}/result.txt", cluster.hubs[0].base_url);
    let body = client.get(get_url).send().await.unwrap().text().await.unwrap();
    assert_eq!(body, "42");

    let stranger_url = format!("{}/storage/nobody/result.txt", cluster.hubs[1].base_url);
    let stranger = client.put(stranger_url).body("x").send().await.unwrap();
    assert_eq!(stranger.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn storage_refuses_an_upload_for_an_agent_that_left_another_hub() {
    let cluster = cluster(2);
    let [(alice, alice_id), (mut bob, _bob_id)] = <[_; 2]>::try_from(one_agent_per_hub(&cluster).await).unwrap();
    drop(alice);
    wait_for_state(&mut bob, &alice_id, &AgentConnectionState::Disconnected).await;

    let put_url = format!("{}/storage/{alice_id}/result.txt", cluster.hubs[1].base_url);
    let put = reqwest::Client::new().put(put_url).body("42").send().await.unwrap();
    assert_eq!(put.status(), reqwest::StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn thirty_agents_spread_over_three_hubs_all_reach_each_other() {
    const AGENTS: usize = 30;
    let cluster = cluster(3);
    // Agents are dealt round the hubs, so neighbours in this list always sit on different hubs.
    let mut sockets = Vec::with_capacity(AGENTS);
    let mut ids = Vec::with_capacity(AGENTS);
    for hub in cluster.hubs.iter().cycle().take(AGENTS) {
        let (socket, id) = connect_agent(&hub.ws_url).await;
        sockets.push(socket);
        ids.push(id);
    }
    // Each agent writes to the next one round the ring, and so hears from the one before it.
    let recipients = ids.iter().cycle().skip(1).take(AGENTS);
    let senders = ids
        .iter()
        .cycle()
        .skip(AGENTS - 1)
        .take(AGENTS)
        .cloned()
        .collect::<Vec<_>>();

    for ((socket, own_id), recipient) in sockets.iter_mut().zip(&ids).zip(recipients) {
        send_direct(socket, recipient, serde_json::json!({"from": own_id})).await;
    }
    let mut exchanged = Vec::with_capacity(AGENTS);
    for (socket, sender) in sockets.iter_mut().zip(&senders) {
        exchanged.push(delivered_and_received(socket, sender).await);
    }
    for (socket, (_, received_id)) in sockets.iter_mut().zip(&exchanged) {
        acknowledge(socket, received_id).await;
    }
    for (socket, sent_and_received) in sockets.iter_mut().zip(exchanged) {
        let expected = BTreeSet::from(<[String; 2]>::from(sent_and_received));
        assert_eq!(two_acknowledged(socket).await, expected);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_remaining_hubs_carry_on_when_one_shuts_down() {
    let cluster = cluster(2);
    let (_alice, alice_id) = connect_agent(&cluster.hubs[0].ws_url).await;
    let (mut bob, _) = connect_agent(&cluster.hubs[1].ws_url).await;
    let (mut carol, carol_id) = connect_agent(&cluster.hubs[1].ws_url).await;

    cluster.hubs[0].stop().await;

    wait_for_state(&mut bob, &alice_id, &AgentConnectionState::Disconnected).await;
    deliver(&mut bob, &mut carol, &carol_id, &serde_json::json!({"still": "here"})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_broker_is_reported_to_the_agent_rather_than_hidden() {
    let cluster = cluster(1);
    let (mut alice, _) = connect_agent(&cluster.hubs[0].ws_url).await;
    drop(cluster.nats);

    send_json(&mut alice, &serde_json::json!({"type": "et-list-agents"})).await;
    expect_backplane_unavailable(&mut alice).await;
    send_direct(&mut alice, "anyone", serde_json::json!({})).await;
    expect_backplane_unavailable(&mut alice).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requests_no_hub_can_honour_are_refused_as_a_lone_hub_would() {
    let cluster = cluster(2);
    let [(mut alice, _), (mut bob, _)] = <[_; 2]>::try_from(one_agent_per_hub(&cluster).await).unwrap();

    send_direct(&mut alice, "nobody", serde_json::json!({})).await;
    assert_eq!(next_invalid(&mut alice).await, "unknown target agent nobody");
    acknowledge(&mut bob, "never-sent").await;
    assert_eq!(
        next_invalid(&mut bob).await,
        "no pending message never-sent to acknowledge"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ack_is_confirmed_to_its_recipient_after_the_sender_has_left() {
    let cluster = cluster(2);
    let [(mut alice, alice_id), (mut bob, bob_id)] = <[_; 2]>::try_from(one_agent_per_hub(&cluster).await).unwrap();
    let body = serde_json::json!({"bye": true});

    let sent_id = send_and_expect(&mut alice, &bob_id, &body, MessageDeliveryStatus::Delivered).await;
    assert_eq!(next_agent_message(&mut bob).await.0, sent_id);
    drop(alice);
    wait_for_state(&mut bob, &alice_id, &AgentConnectionState::Disconnected).await;

    acknowledge(&mut bob, &sent_id).await;
    assert_eq!(
        next_status(&mut bob).await,
        (MessageDeliveryStatus::Acknowledged, sent_id)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lone_hub_stops_cleanly_without_a_backplane() {
    let hub = et_ws_test_server::start();
    let (_alice, _) = connect_agent(&hub.ws_url).await;

    hub.stop().await;
    let _refused = tokio_tungstenite::connect_async(&hub.ws_url).await.unwrap_err();
}
