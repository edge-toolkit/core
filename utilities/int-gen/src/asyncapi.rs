//! `AsyncAPI` spec emission for the WS protocol.
//!
//! The Rust source of truth is `edge_toolkit::ws::{ClientMessage, ServerMessage}`. This module wires those enums
//! through the `asyncapi-rust` derive macros and post-processes the two halves into a single merged `AsyncAPI` 3.0
//! document.
//!
//! The version and description literals on the `#[asyncapi(...)]` derive attributes are mirrored as [`WS_VERSION`] and
//! [`WS_DESCRIPTION`] constants so the WIT and KDL generators can reference the same values; [`build_spec`] asserts
//! they're in sync at runtime.

use asyncapi_rust::AsyncApi;
use edge_toolkit::ws::{ClientMessage, ServerMessage};

use crate::Error;

/// Wire-protocol version.
///
/// Mirrors the `version = ...` literal on `WsApiClient`'s `#[asyncapi(...)]` derive below; if you bump one, bump the
/// other (and the runtime check in [`build_spec`] will catch you if you don't).
pub const WS_VERSION: &str = "0.2.0";

/// Wire-protocol description.
///
/// Mirrors the `description = ...` literal on `WsApiClient`'s `#[asyncapi(...)]` derive below.
pub const WS_DESCRIPTION: &str =
    "Edge Toolkit WS protocol -- typed et-* messages plus relay envelopes for foreign frames.";

/// `AsyncAPI` doc for the ws-server's `/ws` hub channel.
///
/// Split into client + server derives so each operation references only the variants it can legally carry. The two
/// halves are merged in [`build_spec`]; `WsApiClient` provides the surviving `info` block.
#[derive(AsyncApi)]
#[asyncapi(
    title = "Edge Toolkit WebSocket Protocol",
    version = "0.2.0",
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
    version = "0.2.0",
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
///   2. Merge the server-side channels / operations / `components.messages` / `components.schemas` into the
///      client-side document (`merge_asyncapi`).
///   3. Attach each half's messages to its operation and to the channel that operation names (`link_messages`).
///   4. Describe the relay messages as the raw frames they travel as (`describe_relay_frames`).
///   5. Assert that `info.version` matches [`WS_VERSION`] so any drift between the derive literal and the const is
///      caught loudly.
///
/// `asyncapi-rust` 0.5 already emits each message's payload as its own object schema (no `oneOf` fan-out), with shared
/// definitions hoisted into `components.schemas` and canonical `#/components/schemas/...` refs, so no payload-slimming
/// post-processing is needed -- unlike 0.2, which required it.
pub fn build_spec() -> Result<serde_json::Value, Error> {
    let client_spec = WsApiClient::asyncapi_spec();
    let server_spec = WsApiServer::asyncapi_spec();
    let mut spec_value = serde_json::to_value(&client_spec)?;
    let server_value = serde_json::to_value(&server_spec)?;
    let client_value = spec_value.clone();
    merge_asyncapi(&mut spec_value, &server_value);
    link_messages(&mut spec_value, [&client_value, &server_value])?;
    describe_relay_frames(&mut spec_value)?;
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

/// Attach each half's messages to its operations, and list them under the channel each operation names.
///
/// `asyncapi-rust` puts a derive's messages under `components.messages` only, which leaves the merged document's
/// operations and channel carrying no messages: a tool rendering it shows two operations with nothing in them. A half
/// derives exactly the messages its operation carries, so its `components.messages` keys are that operation's list.
#[expect(
    clippy::single_call_fn,
    reason = "named step of build_spec(); the linking is a logical step worth its own scope, like merge_asyncapi"
)]
fn link_messages(spec: &mut serde_json::Value, halves: [&serde_json::Value; 2]) -> Result<(), Error> {
    use serde_json::{Map, Value, json};

    for half in halves {
        let names = message_names(half)?;
        let operations = half
            .get("operations")
            .and_then(Value::as_object)
            .ok_or(Error::SchemaMalformed("an AsyncAPI half has no operations"))?;
        for (operation_name, operation) in operations {
            let channel_ref = operation
                .pointer("/channel/$ref")
                .and_then(Value::as_str)
                .ok_or(Error::SchemaMalformed("an AsyncAPI operation names no channel"))?;
            let channel_messages = spec
                .pointer_mut(channel_ref.trim_start_matches('#'))
                .and_then(Value::as_object_mut)
                .ok_or(Error::SchemaMalformed(
                    "an AsyncAPI operation names a channel the document lacks",
                ))?
                .entry("messages")
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()
                .ok_or(Error::SchemaMalformed(
                    "an AsyncAPI channel's messages is not an object",
                ))?;
            for name in &names {
                let _previous = channel_messages.insert(
                    (*name).clone(),
                    json!({ "$ref": format!("#/components/messages/{name}") }),
                );
            }
            let references = names
                .iter()
                .map(|name| json!({ "$ref": format!("{channel_ref}/messages/{name}") }))
                .collect();
            let _previous = spec
                .pointer_mut(&format!("/operations/{operation_name}"))
                .and_then(Value::as_object_mut)
                .ok_or(Error::SchemaMalformed(
                    "an AsyncAPI operation is missing from the merged document",
                ))?
                .insert("messages".to_owned(), Value::Array(references));
        }
    }
    Ok(())
}

/// The names of the messages an `AsyncAPI` half derives, sorted.
///
/// `serde_json`'s map keeps insertion order when any crate in the build turns on its `preserve_order` feature, and
/// sorts keys otherwise, so the order of its keys depends on what else was compiled with et-int-gen. Sorting here makes
/// the operations' lists the same whichever build writes the spec.
#[expect(
    clippy::single_call_fn,
    reason = "the sorting step of link_messages, kept apart so its reason sits with it"
)]
fn message_names(half: &serde_json::Value) -> Result<Vec<&String>, Error> {
    let mut names: Vec<&String> = half
        .pointer("/components/messages")
        .and_then(serde_json::Value::as_object)
        .ok_or(Error::SchemaMalformed("an AsyncAPI half has no components.messages"))?
        .keys()
        .collect();
    names.sort_unstable();
    Ok(names)
}

/// Describe the two relay messages as the raw frames they travel as, not the JSON envelope the derive gives them.
///
/// `RelayText` and `RelayBinary` are how the hub holds a frame that is not an `et-` message. The derive renders them
/// like every other variant, as a JSON object with `type` and `content`, but the wire carries only the content: the
/// text or binary frame as it arrived, which the hub forwards unchanged.
#[expect(
    clippy::single_call_fn,
    reason = "named step of build_spec(); the relay correction is a logical step worth its own scope"
)]
fn describe_relay_frames(spec: &mut serde_json::Value) -> Result<(), Error> {
    use serde_json::{Value, json};

    let frames = [
        (
            "RelayText",
            "text/plain",
            json!({ "type": "string", "description": "Any text frame that is not an et- message, forwarded as is." }),
        ),
        (
            "RelayBinary",
            "application/octet-stream",
            json!({ "type": "string", "format": "binary", "description": "Any binary frame, forwarded as is." }),
        ),
    ];
    for (name, content_type, payload) in frames {
        let message = spec
            .pointer_mut(&format!("/components/messages/{name}"))
            .and_then(Value::as_object_mut)
            .ok_or(Error::SchemaMalformed(
                "a relay message is missing from the AsyncAPI document",
            ))?;
        let _previous_content_type = message.insert("contentType".to_owned(), json!(content_type));
        let _previous_payload = message.insert("payload".to_owned(), payload);
    }
    Ok(())
}

/// Fold `source`'s channels, operations, `components.messages` and `components.schemas` into `target`.
///
/// Top-level metadata (title, info, servers) is retained from `target`. Used to combine the client-side and server-side
/// `AsyncAPI` documents into a single spec with both directions on one channel.
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
