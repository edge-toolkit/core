//! `ClientMessage::from_text_frame` and `ServerMessage::from_text_frame` are the on-recv decoders shared by the
//! ws-server and the ws-wasi-runner host respectively. Per the protocol design: a frame whose JSON has `type` starting
//! with `et-` is ours and must deserialise; anything else (non-JSON, JSON without a `type`, JSON with a non-et `type`)
//! is foreign and surfaces as `RelayText` so the hub-relay path through the ws-server is lossless. These tests assert
//! that every plausible "deserialisation problem" relays cleanly on both sides rather than failing. The
//! `et-client-event` envelope is also checked against the official `CloudEvents` SDK, in both directions.

#![cfg(test)]
#![expect(
    clippy::wildcard_enum_match_arm,
    reason = "test code: wildcard enum match arms are intentional"
)]

use cloudevents::event::SpecVersion;
use cloudevents::{AttributesReader as _, Data, Event, EventBuilder as _, EventBuilderV10};
use edge_toolkit::ws::{ClientMessage, CloudEvent, ServerMessage, client_event_type};
use serde_json::json;

/// Pull the `content` out of `ClientMessage::RelayText`, panicking if the decoder routed the input elsewhere.
///
/// `ClientMessage` is the server-side decoder -- the server sees client traffic in this shape.
fn client_expect_relay_text(msg: ClientMessage) -> String {
    match msg {
        ClientMessage::RelayText { content } => content,
        other => panic!("expected ClientMessage::RelayText for relay, got {other:?}"),
    }
}

#[test]
fn client_relays_empty_string() {
    let msg = ClientMessage::from_text_frame("").unwrap();
    assert_eq!(client_expect_relay_text(msg), "");
}

#[test]
fn client_relays_plain_text() {
    let msg = ClientMessage::from_text_frame("hello world").unwrap();
    assert_eq!(client_expect_relay_text(msg), "hello world");
}

#[test]
fn client_relays_malformed_json() {
    let msg = ClientMessage::from_text_frame("{not json").unwrap();
    assert_eq!(client_expect_relay_text(msg), "{not json");
}

#[test]
fn client_relays_json_number() {
    let msg = ClientMessage::from_text_frame("42").unwrap();
    assert_eq!(client_expect_relay_text(msg), "42");
}

#[test]
fn client_relays_json_string_literal() {
    let raw = "\"hello\"";
    let msg = ClientMessage::from_text_frame(raw).unwrap();
    assert_eq!(client_expect_relay_text(msg), raw);
}

#[test]
fn client_relays_json_array() {
    let msg = ClientMessage::from_text_frame("[1, 2, 3]").unwrap();
    assert_eq!(client_expect_relay_text(msg), "[1, 2, 3]");
}

#[test]
fn client_relays_json_null() {
    let msg = ClientMessage::from_text_frame("null").unwrap();
    assert_eq!(client_expect_relay_text(msg), "null");
}

#[test]
fn client_relays_json_object_without_type() {
    let raw = r#"{"hello":"world"}"#;
    let msg = ClientMessage::from_text_frame(raw).unwrap();
    assert_eq!(client_expect_relay_text(msg), raw);
}

#[test]
fn client_relays_json_object_with_non_string_type() {
    let raw = r#"{"type":42,"payload":true}"#;
    let msg = ClientMessage::from_text_frame(raw).unwrap();
    assert_eq!(client_expect_relay_text(msg), raw);
}

#[test]
fn client_relays_json_object_with_non_et_type() {
    let raw = r#"{"type":"foo-bar","x":1}"#;
    let msg = ClientMessage::from_text_frame(raw).unwrap();
    assert_eq!(client_expect_relay_text(msg), raw);
}

#[test]
fn client_relays_json_object_with_type_et_no_dash() {
    let raw = r#"{"type":"etwhatever"}"#;
    let msg = ClientMessage::from_text_frame(raw).unwrap();
    assert_eq!(client_expect_relay_text(msg), raw);
}

#[test]
fn client_relays_json_object_with_capitalised_et_prefix() {
    let raw = r#"{"type":"Et-connect"}"#;
    let msg = ClientMessage::from_text_frame(raw).unwrap();
    assert_eq!(client_expect_relay_text(msg), raw);
}

#[test]
fn client_relays_third_party_vendor_prefix() {
    let raw = r#"{"type":"vendor-x-event","seq":7}"#;
    let msg = ClientMessage::from_text_frame(raw).unwrap();
    assert_eq!(client_expect_relay_text(msg), raw);
}

#[test]
fn client_typed_for_valid_et_message() {
    // `et-list-agents` has no payload fields; the bare envelope parses.
    let msg = ClientMessage::from_text_frame(r#"{"type":"et-list-agents"}"#).unwrap();
    assert!(
        matches!(msg, ClientMessage::ListAgents),
        "expected ClientMessage::ListAgents, got {msg:?}"
    );
}

/// A `CloudEvent` with fixed `id` and `time`, so the wire form it serialises to can be written out in full.
fn sample_event(source: &str, capability: &str, action: &str, data: serde_json::Value) -> CloudEvent {
    CloudEvent::new(
        "event-1".to_owned(),
        source.to_owned(),
        client_event_type(capability, action),
        "2026-10-06T00:00:00Z".parse().unwrap(),
        data,
    )
}

/// Decode `event`'s wire form as the official `CloudEvents` SDK reads it.
fn decoded_by_sdk(event: &CloudEvent) -> Event {
    serde_json::from_value(serde_json::to_value(event).unwrap()).unwrap()
}

#[test]
fn a_client_event_carries_its_cloudevent_nested_under_event() {
    let event = sample_event("/modules/test", "app", "loaded", json!({ "build": "test" }));
    let frame = serde_json::to_value(ClientMessage::ClientEvent { event: event.clone() }).unwrap();
    assert_eq!(
        frame,
        json!({
            "type": "et-client-event",
            "event": {
                "data": { "build": "test" },
                "id": "event-1",
                "source": "/modules/test",
                "specversion": "1.0",
                "time": "2026-10-06T00:00:00Z",
                "type": "et.app.loaded",
            },
        })
    );
    match ClientMessage::from_text_frame(&frame.to_string()).unwrap() {
        ClientMessage::ClientEvent { event: decoded } => assert_eq!(decoded, event),
        other => panic!("expected ClientMessage::ClientEvent, got {other:?}"),
    }
}

#[test]
fn a_client_event_without_its_cloudevent_time_is_a_decode_error() {
    let frame = json!({
        "type": "et-client-event",
        "event": { "data": {}, "id": "event-1", "source": "/modules/test", "specversion": "1.0", "type": "et.a.b" },
    });
    let _err = ClientMessage::from_text_frame(&frame.to_string()).unwrap_err();
}

/// The text of a sample `et-client-event` frame whose event attribute `field` is written as `value`.
fn frame_with(field: &str, value: &str) -> String {
    let mut frame = serde_json::to_value(ClientMessage::ClientEvent {
        event: sample_event("/modules/test", "app", "loaded", json!({})),
    })
    .unwrap();
    frame["event"][field] = json!(value);
    frame.to_string()
}

#[test]
fn a_client_event_whose_time_is_not_a_timestamp_is_a_decode_error() {
    let _err = ClientMessage::from_text_frame(&frame_with("time", "yesterday")).unwrap_err();
}

#[test]
fn a_client_event_at_an_unsupported_specversion_is_a_decode_error() {
    let _err = ClientMessage::from_text_frame(&frame_with("specversion", "0.3")).unwrap_err();
}

#[test]
fn a_client_event_time_in_another_offset_decodes_to_the_same_instant() {
    match ClientMessage::from_text_frame(&frame_with("time", "2026-10-06T10:00:00+10:00")).unwrap() {
        ClientMessage::ClientEvent { event } => assert_eq!(event.time, sample_event("", "", "", json!({})).time),
        other => panic!("expected ClientMessage::ClientEvent, got {other:?}"),
    }
}

#[test]
fn an_envelope_this_protocol_sends_is_a_cloudevent_to_the_sdk() {
    let decoded = decoded_by_sdk(&sample_event(
        "/modules/test",
        "app",
        "loaded",
        json!({ "build": "test" }),
    ));

    assert_eq!(decoded.specversion(), SpecVersion::V10);
    assert_eq!(decoded.id(), "event-1");
    assert_eq!(decoded.source(), "/modules/test");
    assert_eq!(decoded.ty(), "et.app.loaded");
    assert_eq!(decoded.time().unwrap().to_rfc3339(), "2026-10-06T00:00:00+00:00");
    assert_eq!(decoded.data(), Some(&Data::Json(json!({ "build": "test" }))));
}

#[test]
fn a_served_npm_module_path_source_names_the_invoking_crate_rather_than_et_org() {
    let source = et_org::served_npm_module_path!();
    assert_eq!(source, "/modules/@edge-toolkit/edge-toolkit");

    let decoded = decoded_by_sdk(&sample_event(source, "app", "loaded", json!({})));
    assert_eq!(decoded.source(), "/modules/@edge-toolkit/edge-toolkit");
}

#[test]
fn an_event_the_sdk_builds_decodes_as_this_protocols_envelope() {
    let built = EventBuilderV10::new()
        .id("event-1")
        .source("/modules/test")
        .ty("et.consent.upload_consent_changed")
        .time("2026-10-06T00:00:00Z")
        .data("application/json", json!({ "checked": true }))
        .build()
        .unwrap();
    let decoded: CloudEvent = serde_json::from_value(serde_json::to_value(&built).unwrap()).unwrap();

    let expected = sample_event(
        "/modules/test",
        "consent",
        "upload_consent_changed",
        json!({ "checked": true }),
    );
    assert_eq!(decoded, expected);
}

#[test]
fn client_typed_for_server_only_variant_is_decode_error() {
    // `et-connect-ack` lives in ServerMessage, not ClientMessage. A client claiming to send a ConnectAck must surface
    // as a decode error -- that's the type-level enforcement the split exists to provide.
    let _err =
        ClientMessage::from_text_frame(r#"{"type":"et-connect-ack","agent_id":"a","status":"assigned"}"#).unwrap_err();
}

#[test]
fn client_decode_error_for_unknown_variant() {
    let _err = ClientMessage::from_text_frame(r#"{"type":"et-bogus-variant"}"#).unwrap_err();
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
    let msg = ServerMessage::from_text_frame("hello").unwrap();
    assert_eq!(server_expect_relay_text(msg), "hello");
}

#[test]
fn server_relays_json_object_with_non_et_type() {
    let raw = r#"{"type":"vendor-y-broadcast","seq":1}"#;
    let msg = ServerMessage::from_text_frame(raw).unwrap();
    assert_eq!(server_expect_relay_text(msg), raw);
}

#[test]
fn server_typed_for_response_variant() {
    let msg = ServerMessage::from_text_frame(r#"{"type":"et-response","message":"hi"}"#).unwrap();
    match msg {
        ServerMessage::Response { message } => assert_eq!(message, "hi"),
        other => panic!("expected ServerMessage::Response, got {other:?}"),
    }
}

#[test]
fn server_typed_for_client_only_variant_is_decode_error() {
    // `et-connect` lives in ClientMessage. A server claiming to send Connect to a client must surface as a decode
    // error.
    let _err = ServerMessage::from_text_frame(r#"{"type":"et-connect"}"#).unwrap_err();
}

#[test]
fn server_binary_frame_always_relays() {
    let bytes = vec![10_u8, 20, 30];
    match ServerMessage::from_binary_frame(bytes.clone()) {
        ServerMessage::RelayBinary { content } => assert_eq!(content, bytes),
        other => panic!("expected ServerMessage::RelayBinary, got {other:?}"),
    }
}
