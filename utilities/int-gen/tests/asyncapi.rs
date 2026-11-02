//! Covers `asyncapi::build_spec`: each operation, and the channel, carries the messages of the side that sends them.
#![cfg(test)]

use et_int_gen::asyncapi::build_spec;
use serde_json::{Value, json};

/// The names of the messages an operation's `messages` list references, in order.
fn operation_messages(spec: &Value, operation: &str) -> Vec<String> {
    spec.pointer(&format!("/operations/{operation}/messages"))
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .map(|reference| {
            reference["$ref"]
                .as_str()
                .unwrap()
                .trim_start_matches("#/channels/ws/messages/")
                .to_owned()
        })
        .collect()
}

#[test]
fn the_send_operation_carries_every_client_message() {
    let spec = build_spec().unwrap();

    assert_eq!(
        operation_messages(&spec, "sendWsMessage"),
        [
            "Alive",
            "BroadcastMessage",
            "ClientEvent",
            "Connect",
            "ListAgents",
            "MessageAck",
            "RelayBinary",
            "RelayText",
            "SendAgentMessage",
        ]
    );
}

#[test]
fn the_receive_operation_carries_every_server_message() {
    let spec = build_spec().unwrap();

    assert_eq!(
        operation_messages(&spec, "receiveWsMessage"),
        [
            "AgentMessage",
            "ConnectAck",
            "Invalid",
            "ListAgentsResponse",
            "MessageStatus",
            "RelayBinary",
            "RelayText",
            "Response",
        ]
    );
}

#[test]
fn the_relay_messages_are_described_as_raw_frames() {
    let spec = build_spec().unwrap();
    let text = spec.pointer("/components/messages/RelayText").unwrap();
    let binary = spec.pointer("/components/messages/RelayBinary").unwrap();

    assert_eq!(text["contentType"], "text/plain");
    assert_eq!(text["payload"]["type"], "string");
    assert_eq!(binary["contentType"], "application/octet-stream");
    assert_eq!(binary["payload"]["format"], "binary");
}

#[test]
fn the_channel_lists_every_message_by_its_component() {
    let spec = build_spec().unwrap();
    let channel = spec
        .pointer("/channels/ws/messages")
        .and_then(Value::as_object)
        .unwrap();

    assert_eq!(channel.len(), 15);
    assert_eq!(channel["Connect"], json!({ "$ref": "#/components/messages/Connect" }));
    assert_eq!(channel["Response"], json!({ "$ref": "#/components/messages/Response" }));
}
