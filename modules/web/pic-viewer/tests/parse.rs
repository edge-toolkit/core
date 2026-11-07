//! Tests for `parse_capture_notification`, the frame filter feeding the pic-viewer's display queue.
//!
//! The parser is the module's whole trust boundary: every WebSocket frame the viewer's client receives goes
//! through it, and only what it returns ever reaches `show_image`. These run natively (no browser) because
//! the function is pure string-to-struct decoding.

#![cfg(test)]

use et_ws_pic_viewer::parse_capture_notification;
use serde_json::json;

/// The `et-agent-message` envelope from a live end-to-end run, pinning the real wire format.
///
/// Captured from the server's fan-out of a pyeye1 `pyeye1_capture_stored` broadcast; verbatim except for the
/// added line breaks between JSON tokens, which are insignificant whitespace to the parser.
const LIVE_BROADCAST_FRAME: &str = r#"{"type":"et-agent-message",
"message_id":"019f8c4a-2603-76e1-a8f3-3bd662e809e7",
"from_agent_id":"019f8c4a-25f5-7162-a35a-db901665cefb",
"scope":"broadcast",
"server_received_at":"2026-07-23T00:04:57.475093+00:00",
"message":{"agent_id":"019f8c4a-25f5-7162-a35a-db901665cefb",
"filename":"pyeye1-eye-capture-demo.png",
"kind":"pyeye1_capture_stored",
"url":"/storage/019f8c4a-25f5-7162-a35a-db901665cefb/pyeye1-eye-capture-demo.png"}}"#;

/// Wrap a payload in a well-formed `et-agent-message` envelope, the shape the server relays broadcasts in.
fn envelope(payload: &serde_json::Value) -> String {
    json!({
        "type": "et-agent-message",
        "message_id": "test-message-id",
        "from_agent_id": "agent-sender",
        "scope": "broadcast",
        "server_received_at": "2026-07-23T00:00:00+00:00",
        "message": payload,
    })
    .to_string()
}

/// A fully valid capture announcement payload for tests to mutate.
fn valid_payload() -> serde_json::Value {
    json!({
        "kind": "pyeye1_capture_stored",
        "agent_id": "agent-sender",
        "filename": "capture.png",
        "url": "/storage/agent-sender/capture.png",
    })
}

#[test]
fn decodes_the_live_broadcast_envelope() {
    let notification = parse_capture_notification(LIVE_BROADCAST_FRAME).unwrap();
    assert_eq!(notification.from_agent_id, "019f8c4a-25f5-7162-a35a-db901665cefb");
    assert_eq!(notification.filename, "pyeye1-eye-capture-demo.png");
    assert_eq!(
        notification.url,
        "/storage/019f8c4a-25f5-7162-a35a-db901665cefb/pyeye1-eye-capture-demo.png"
    );
}

#[test]
fn decodes_a_direct_scope_announcement_too() {
    // A peer could announce via send_agent_message instead of a broadcast; scope is not part of the filter.
    let frame = envelope(&valid_payload()).replace(r#""scope":"broadcast""#, r#""scope":"direct""#);
    let notification = parse_capture_notification(&frame).unwrap();
    assert_eq!(notification.url, "/storage/agent-sender/capture.png");
}

#[test]
fn ignores_non_agent_message_traffic() {
    let other_frames = [
        r#"{"type":"et-connect-ack","agent_id":"agent-1","status":"assigned"}"#,
        r#"{"type":"et-response","message":"Alive message received"}"#,
        r#"{"type":"et-message-status","message_id":"m-1","status":"broadcast","detail":"sent"}"#,
        "not json at all",
        "",
        "42",
    ];
    for frame in other_frames {
        assert!(
            parse_capture_notification(frame).is_none(),
            "frame should be ignored: {frame}"
        );
    }
}

#[test]
fn ignores_other_broadcast_kinds() {
    let mut wrong_kind = valid_payload();
    wrong_kind["kind"] = json!("some_other_module_event");
    assert!(parse_capture_notification(&envelope(&wrong_kind)).is_none());

    let mut no_kind = valid_payload();
    let _removed = no_kind.as_object_mut().unwrap().remove("kind");
    assert!(parse_capture_notification(&envelope(&no_kind)).is_none());
}

#[test]
fn rejects_missing_or_non_string_urls() {
    let mut no_url = valid_payload();
    let _removed = no_url.as_object_mut().unwrap().remove("url");
    assert!(parse_capture_notification(&envelope(&no_url)).is_none());

    let mut numeric_url = valid_payload();
    numeric_url["url"] = json!(42_i32);
    assert!(parse_capture_notification(&envelope(&numeric_url)).is_none());
}

#[test]
fn rejects_urls_outside_same_origin_storage() {
    // Announcements arrive from arbitrary peers; anything that could steer the viewer off the same-origin
    // storage tree must be dropped, including protocol-relative forms.
    let hostile_urls = [
        "https://evil.example/steal.png",
        "http://evil.example/steal.png",
        "//evil.example/steal.png",
        "/other/path.png",
        "storage/relative.png",
        "javascript:alert(1)",
    ];
    for url in hostile_urls {
        let mut payload = valid_payload();
        payload["url"] = json!(url);
        assert!(
            parse_capture_notification(&envelope(&payload)).is_none(),
            "url should be rejected: {url}"
        );
    }
}

#[test]
fn missing_filename_defaults_to_empty() {
    let mut no_filename = valid_payload();
    let _removed = no_filename.as_object_mut().unwrap().remove("filename");
    let notification = parse_capture_notification(&envelope(&no_filename)).unwrap();
    assert_eq!(notification.filename, "");
}
