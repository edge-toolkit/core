//! Two connections claiming one agent id: the newest claim wins, and the connection it displaced cannot take the id
//! offline when it later closes.
#![cfg(test)]

use std::time::Duration;

use edge_toolkit::ws::ServerMessage;
use et_ws_test_server::{connect_agent, connect_agent_as, next_payload};
use futures_util::{SinkExt as _, StreamExt as _};
use tokio_tungstenite::tungstenite::Message;

/// The id both connections below claim, as an agent naming itself would.
const SHARED_ID: &str = "shared-agent";

type Stream = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Send `payload` from `sender` to the shared id, and return the payload `recipient` receives.
async fn relay_to_shared(sender: &mut Stream, recipient: &mut Stream, payload: u32) -> serde_json::Value {
    let send = serde_json::json!({
        "type": "et-send-agent-message",
        "to_agent_id": SHARED_ID,
        "message": {"n": payload},
    });
    sender.send(Message::text(send.to_string())).await.unwrap();
    let Message::Text(text) = next_payload(recipient).await else {
        panic!("expected an et-agent-message text frame");
    };
    let ServerMessage::AgentMessage { message, .. } = serde_json::from_str::<ServerMessage>(&text).unwrap() else {
        panic!("expected ServerMessage::AgentMessage, got {text}");
    };
    message
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_newest_claim_on_an_id_survives_the_displaced_connection_closing() {
    let server = et_ws_test_server::start();
    let (mut displaced, first_id) = connect_agent_as(&server.ws_url, Some(SHARED_ID)).await;
    let (mut newest, second_id) = connect_agent_as(&server.ws_url, Some(SHARED_ID)).await;
    let (mut sender, _sender_id) = connect_agent(&server.ws_url).await;
    assert_eq!(first_id, SHARED_ID);
    assert_eq!(second_id, SHARED_ID);

    // Direct messages to the id reach the connection that claimed it last.
    assert_eq!(
        relay_to_shared(&mut sender, &mut newest, 1).await,
        serde_json::json!({"n": 1_u32})
    );

    // The displaced connection closes. Reading it to the end waits for the hub to answer the close, after which its
    // handler marks the id disconnected; the pause covers the few instructions between the two.
    displaced.send(Message::Close(None)).await.unwrap();
    while let Ok(Some(Ok(_frame))) = tokio::time::timeout(Duration::from_secs(5), displaced.next()).await {}
    tokio::time::sleep(Duration::from_millis(250)).await;

    // Taking the id offline here would drop the newest connection's session, and this message would sit queued.
    assert_eq!(
        relay_to_shared(&mut sender, &mut newest, 2).await,
        serde_json::json!({"n": 2_u32})
    );
}
