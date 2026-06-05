//! `ClientMessage::from_text_frame` and `ServerMessage::from_text_frame`
//! are the on-recv decoders shared by the ws-server and the
//! ws-wasi-runner host respectively. Per the protocol design: a frame
//! whose JSON has `type` starting with `et-` is ours and must
//! deserialise; anything else (non-JSON, JSON without a `type`, JSON
//! with a non-et `type`) is foreign and surfaces as `RelayText` so the
//! hub-relay path through the ws-server is lossless. These tests assert
//! that every plausible "deserialisation problem" relays cleanly on
//! both sides rather than failing.

#![cfg(test)]
#![expect(
    clippy::expect_used,
    clippy::panic,
    clippy::wildcard_enum_match_arm,
    reason = "test code: assertion panics carry enough context for tests"
)]

use edge_toolkit::ws::{ClientMessage, ServerMessage};

/// Pull the `content` out of `ClientMessage::RelayText`, panicking if the
/// decoder routed the input elsewhere. (`ClientMessage` is the server-side
/// decoder — the server sees client traffic in this shape.)
fn client_expect_relay_text(msg: ClientMessage) -> String {
    match msg {
        ClientMessage::RelayText { content } => content,
        other => panic!("expected ClientMessage::RelayText for relay, got {other:?}"),
    }
}

#[test]
fn client_relays_empty_string() {
    let msg = ClientMessage::from_text_frame("").expect("relay must not error");
    assert_eq!(client_expect_relay_text(msg), "");
}

#[test]
fn client_relays_plain_text() {
    let msg = ClientMessage::from_text_frame("hello world").expect("relay must not error");
    assert_eq!(client_expect_relay_text(msg), "hello world");
}

#[test]
fn client_relays_malformed_json() {
    let msg = ClientMessage::from_text_frame("{not json").expect("relay must not error");
    assert_eq!(client_expect_relay_text(msg), "{not json");
}

#[test]
fn client_relays_json_number() {
    let msg = ClientMessage::from_text_frame("42").expect("relay must not error");
    assert_eq!(client_expect_relay_text(msg), "42");
}

#[test]
fn client_relays_json_string_literal() {
    let raw = "\"hello\"";
    let msg = ClientMessage::from_text_frame(raw).expect("relay must not error");
    assert_eq!(client_expect_relay_text(msg), raw);
}

#[test]
fn client_relays_json_array() {
    let msg = ClientMessage::from_text_frame("[1, 2, 3]").expect("relay must not error");
    assert_eq!(client_expect_relay_text(msg), "[1, 2, 3]");
}

#[test]
fn client_relays_json_null() {
    let msg = ClientMessage::from_text_frame("null").expect("relay must not error");
    assert_eq!(client_expect_relay_text(msg), "null");
}

#[test]
fn client_relays_json_object_without_type() {
    let raw = r#"{"hello":"world"}"#;
    let msg = ClientMessage::from_text_frame(raw).expect("relay must not error");
    assert_eq!(client_expect_relay_text(msg), raw);
}

#[test]
fn client_relays_json_object_with_non_string_type() {
    let raw = r#"{"type":42,"payload":true}"#;
    let msg = ClientMessage::from_text_frame(raw).expect("relay must not error");
    assert_eq!(client_expect_relay_text(msg), raw);
}

#[test]
fn client_relays_json_object_with_non_et_type() {
    let raw = r#"{"type":"foo-bar","x":1}"#;
    let msg = ClientMessage::from_text_frame(raw).expect("relay must not error");
    assert_eq!(client_expect_relay_text(msg), raw);
}

#[test]
fn client_relays_json_object_with_type_et_no_dash() {
    let raw = r#"{"type":"etwhatever"}"#;
    let msg = ClientMessage::from_text_frame(raw).expect("relay must not error");
    assert_eq!(client_expect_relay_text(msg), raw);
}

#[test]
fn client_relays_json_object_with_capitalised_et_prefix() {
    let raw = r#"{"type":"Et-connect"}"#;
    let msg = ClientMessage::from_text_frame(raw).expect("relay must not error");
    assert_eq!(client_expect_relay_text(msg), raw);
}

#[test]
fn client_relays_third_party_vendor_prefix() {
    let raw = r#"{"type":"vendor-x-event","seq":7}"#;
    let msg = ClientMessage::from_text_frame(raw).expect("relay must not error");
    assert_eq!(client_expect_relay_text(msg), raw);
}

#[test]
fn client_typed_for_valid_et_message() {
    // `et-list-agents` has no payload fields; the bare envelope parses.
    let msg = ClientMessage::from_text_frame(r#"{"type":"et-list-agents"}"#).expect("typed parse must succeed");
    assert!(
        matches!(msg, ClientMessage::ListAgents),
        "expected ClientMessage::ListAgents, got {msg:?}"
    );
}

#[test]
fn client_typed_for_server_only_variant_is_decode_error() {
    // `et-connect-ack` lives in ServerMessage, not ClientMessage. A client
    // claiming to send a ConnectAck must surface as a decode error — that's
    // the type-level enforcement the split exists to provide.
    let _err = ClientMessage::from_text_frame(r#"{"type":"et-connect-ack","agent_id":"a","status":"assigned"}"#)
        .expect_err("server-side variant in client decoder must surface a decode error");
}

#[test]
fn client_decode_error_for_unknown_variant() {
    let _err = ClientMessage::from_text_frame(r#"{"type":"et-bogus-variant"}"#)
        .expect_err("et-prefixed unknown variants must surface a decode error");
}

#[test]
fn client_binary_frame_always_relays() {
    let bytes = vec![0_u8, 1, 2, 254, 255];
    match ClientMessage::from_binary_frame(bytes.clone()) {
        ClientMessage::RelayBinary { content } => assert_eq!(content, bytes),
        other => panic!("expected ClientMessage::RelayBinary, got {other:?}"),
    }
}

// --- ServerMessage (client-side decoder) ---------------------------------

fn server_expect_relay_text(msg: ServerMessage) -> String {
    match msg {
        ServerMessage::RelayText { content } => content,
        other => panic!("expected ServerMessage::RelayText for relay, got {other:?}"),
    }
}

#[test]
fn server_relays_plain_text() {
    let msg = ServerMessage::from_text_frame("hello").expect("relay must not error");
    assert_eq!(server_expect_relay_text(msg), "hello");
}

#[test]
fn server_relays_json_object_with_non_et_type() {
    let raw = r#"{"type":"vendor-y-broadcast","seq":1}"#;
    let msg = ServerMessage::from_text_frame(raw).expect("relay must not error");
    assert_eq!(server_expect_relay_text(msg), raw);
}

#[test]
fn server_typed_for_response_variant() {
    let msg =
        ServerMessage::from_text_frame(r#"{"type":"et-response","message":"hi"}"#).expect("typed parse must succeed");
    match msg {
        ServerMessage::Response { message } => assert_eq!(message, "hi"),
        other => panic!("expected ServerMessage::Response, got {other:?}"),
    }
}

#[test]
fn server_typed_for_client_only_variant_is_decode_error() {
    // `et-connect` lives in ClientMessage. A server claiming to send Connect
    // to a client must surface as a decode error.
    let _err = ServerMessage::from_text_frame(r#"{"type":"et-connect"}"#)
        .expect_err("client-side variant in server decoder must surface a decode error");
}

#[test]
fn server_binary_frame_always_relays() {
    let bytes = vec![10_u8, 20, 30];
    match ServerMessage::from_binary_frame(bytes.clone()) {
        ServerMessage::RelayBinary { content } => assert_eq!(content, bytes),
        other => panic!("expected ServerMessage::RelayBinary, got {other:?}"),
    }
}
