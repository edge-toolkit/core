//! Acknowledging a direct message must notify the original sender: when the recipient sends `et-message-ack`,
//! the hub pushes an `et-message-status` (Acknowledged) back to the sender's still-connected session.
#![cfg(test)]

use std::time::Duration;

use edge_toolkit::ws::{MessageDeliveryStatus, ServerMessage};
use et_ws_test_server::{connect_agent, next_payload};
use futures_util::{SinkExt as _, StreamExt as _};
use tokio_tungstenite::tungstenite::Message;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ack_notifies_original_sender() {
    let server = et_ws_test_server::start();
    let (mut sender, _sender_id) = connect_agent(&server.ws_url).await;
    let (mut recipient, recipient_id) = connect_agent(&server.ws_url).await;

    // Sender -> direct message -> recipient.
    let send = serde_json::json!({
        "type": "et-send-agent-message",
        "to_agent_id": recipient_id,
        "message": {"ping": 1_u32},
    });
    sender.send(Message::text(send.to_string())).await.unwrap();

    // The recipient receives the delivered message; capture its id so it can acknowledge.
    let delivered = next_payload(&mut recipient).await;
    let Message::Text(text) = delivered else {
        panic!("expected an et-agent-message text frame, got {delivered:?}");
    };
    let ServerMessage::AgentMessage { message_id, .. } = serde_json::from_str::<ServerMessage>(&text).unwrap() else {
        panic!("expected ServerMessage::AgentMessage, got {text}");
    };

    // Recipient acknowledges receipt.
    let ack = serde_json::json!({ "type": "et-message-ack", "message_id": message_id });
    recipient.send(Message::text(ack.to_string())).await.unwrap();

    // The sender's stream carries a Delivered status first; read on until the Acknowledged one arrives.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        // Three unwraps mirror `next_payload`: timeout elapsed, stream ended, stream error.
        let msg = tokio::time::timeout(remaining, sender.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let Message::Text(text) = msg else { continue };
        let Ok(ServerMessage::MessageStatus {
            message_id: acked_id,
            status,
            ..
        }) = serde_json::from_str::<ServerMessage>(&text)
        else {
            continue;
        };
        if matches!(status, MessageDeliveryStatus::Acknowledged) {
            assert_eq!(
                acked_id.as_deref(),
                Some(message_id.as_str()),
                "acknowledged status must carry the original message id"
            );
            return;
        }
    }
}
