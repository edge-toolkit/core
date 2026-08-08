//! `AsyncAPI` spec emission for the WS protocol.
//!
//! The Rust source of truth is `edge_toolkit::ws::{ClientMessage,
//! ServerMessage}`. This module wires those enums through the
//! `asyncapi-rust` derive macros and post-processes the two halves into
//! a single merged `AsyncAPI` 3.0 document.
//!
//! The version and description literals on the `#[asyncapi(...)]` derive
//! attributes are mirrored as [`WS_VERSION`] and [`WS_DESCRIPTION`]
//! constants so the WIT and KDL generators can reference the same
//! values; [`build_spec`] asserts they're in sync at runtime.

use asyncapi_rust::AsyncApi;
use edge_toolkit::ws::{ClientMessage, ServerMessage};

use crate::Error;

/// Wire-protocol version.
///
/// Mirrors the `version = ...` literal on `WsApiClient`'s
/// `#[asyncapi(...)]` derive below; if you bump one, bump the other
/// (and the runtime check in [`build_spec`] will catch you if you don't).
pub const WS_VERSION: &str = "0.1.0";

/// Wire-protocol description.
///
/// Mirrors the `description = ...` literal on `WsApiClient`'s
/// `#[asyncapi(...)]` derive below.
pub const WS_DESCRIPTION: &str =
    "Edge Toolkit WS protocol -- typed et-* messages plus relay envelopes for foreign frames.";

/// `AsyncAPI` doc for the ws-server's `/ws` hub channel.
///
/// Split into client + server derives so each operation references only
/// the variants it can legally carry. The two halves are merged in
/// [`build_spec`]; `WsApiClient` provides the surviving `info` block.
#[derive(AsyncApi)]
#[asyncapi(
    title = "Edge Toolkit WebSocket Protocol",
    version = "0.1.0",
    description = "Edge Toolkit WS protocol -- typed et-* messages plus relay envelopes for foreign frames."
)]
#[asyncapi_server(
    name = "local",
    host = "localhost:8080",
    protocol = "ws",
    description = "Default ws-server bind address (mise run ws-server)"
)]
#[asyncapi_channel(name = "ws", address = "/ws")]
#[asyncapi_operation(name = "sendWsMessage", action = "send", channel = "ws")]
#[asyncapi_messages(ClientMessage)]
struct WsApiClient;

#[derive(AsyncApi)]
#[asyncapi(
    title = "Edge Toolkit WebSocket Protocol",
    version = "0.1.0",
    description = "Server-side half -- merged into the client-side spec by int-gen."
)]
#[asyncapi_server(
    name = "local",
    host = "localhost:8080",
    protocol = "ws",
    description = "Default ws-server bind address (mise run ws-server)"
)]
#[asyncapi_channel(name = "ws", address = "/ws")]
#[asyncapi_operation(name = "receiveWsMessage", action = "receive", channel = "ws")]
#[asyncapi_messages(ServerMessage)]
struct WsApiServer;

/// Build the merged `AsyncAPI` spec as a `serde_json::Value`.
///
/// Steps:
///   1. Derive the two halves via `WsApiClient` / `WsApiServer`.
///   2. Merge the server-side channels / operations / messages into the
///      client-side document (`merge_asyncapi`).
///   3. Assert that `info.version` matches [`WS_VERSION`] so any drift
///      between the derive literal and the const is caught loudly.
///
/// `asyncapi-rust` 0.5 already emits each message's payload as its own object
/// schema (no `oneOf` fan-out), with shared definitions hoisted into
/// `components.schemas` and canonical `#/components/schemas/...` refs, so no
/// payload-slimming post-processing is needed -- unlike 0.2, which required it.
pub fn build_spec() -> Result<serde_json::Value, Error> {
    let client_spec = WsApiClient::asyncapi_spec();
    let server_spec = WsApiServer::asyncapi_spec();
    let mut spec_value = serde_json::to_value(&client_spec)?;
    let server_value = serde_json::to_value(&server_spec)?;
    merge_asyncapi(&mut spec_value, &server_value);
    let info_version = spec_value
        .get("info")
        .and_then(|info| info.get("version"))
        .and_then(serde_json::Value::as_str);
    if info_version != Some(WS_VERSION) {
        return Err(Error::SchemaMalformed(
            "WS_VERSION const drifted from WsApiClient's #[asyncapi(version)] literal",
        ));
    }
    Ok(spec_value)
}

/// Fold `source`'s channels, operations, and `components.messages` into
/// `target`. Top-level metadata (title, info, servers) is retained from
/// `target`. Used to combine the client-side and server-side `AsyncAPI`
/// documents into a single spec with both directions on one channel.
#[expect(
    clippy::single_call_fn,
    reason = "named helper called once by build_spec(); the merge is a logical step worth its own scope"
)]
fn merge_asyncapi(target: &mut serde_json::Value, source: &serde_json::Value) {
    use serde_json::Value;
    fn merge_object_field(target: &mut Value, source: &Value, field: &str) {
        let Some(source_field) = source.get(field).and_then(Value::as_object) else {
            return;
        };
        let Some(target_obj) = target.as_object_mut() else {
            return;
        };
        let entry = target_obj
            .entry(field.to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        let Some(entry_obj) = entry.as_object_mut() else {
            return;
        };
        for (key, value) in source_field {
            let _previous: Option<Value> = entry_obj.insert(key.clone(), value.clone());
        }
    }
    fn merge_nested(target: &mut Value, source: &Value, outer: &str, inner: &str) {
        let Some(source_inner) = source
            .get(outer)
            .and_then(|outer_value| outer_value.get(inner))
            .and_then(Value::as_object)
        else {
            return;
        };
        let Some(target_obj) = target.as_object_mut() else {
            return;
        };
        let outer_entry = target_obj
            .entry(outer.to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        let Some(outer_obj) = outer_entry.as_object_mut() else {
            return;
        };
        let inner_entry = outer_obj
            .entry(inner.to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        let Some(inner_obj) = inner_entry.as_object_mut() else {
            return;
        };
        for (key, value) in source_inner {
            let _previous: Option<Value> = inner_obj.insert(key.clone(), value.clone());
        }
    }
    merge_object_field(target, source, "channels");
    merge_object_field(target, source, "operations");
    merge_nested(target, source, "components", "messages");
    merge_nested(target, source, "components", "schemas");
}
