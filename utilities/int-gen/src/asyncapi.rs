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
    "Edge Toolkit WS protocol — typed et-* messages plus relay envelopes for foreign frames.";

/// `AsyncAPI` doc for the ws-server's `/ws` hub channel.
///
/// Split into client + server derives so each operation references only
/// the variants it can legally carry. The two halves are merged in
/// [`build_spec`]; `WsApiClient` provides the surviving `info` block.
#[derive(AsyncApi)]
#[asyncapi(
    title = "Edge Toolkit WebSocket Protocol",
    version = "0.1.0",
    description = "Edge Toolkit WS protocol — typed et-* messages plus relay envelopes for foreign frames."
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
    description = "Server-side half — merged into the client-side spec by int-gen."
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

/// Build the merged, slimmed `AsyncAPI` spec as a `serde_json::Value`.
///
/// Steps:
///   1. Derive the two halves via `WsApiClient` / `WsApiServer`.
///   2. Merge the server-side channels / operations / messages into the
///      client-side document (`merge_asyncapi`).
///   3. Slim each `components.messages.<name>.payload` to just its
///      tagged variant and hoist shared `$defs` into
///      `components.schemas` (`slim_component_messages`).
///   4. Assert that `info.version` matches [`WS_VERSION`] so any drift
///      between the derive literal and the const is caught loudly.
pub fn build_spec() -> Result<serde_json::Value, Error> {
    let client_spec = WsApiClient::asyncapi_spec();
    let server_spec = WsApiServer::asyncapi_spec();
    let mut spec_value = serde_json::to_value(&client_spec)?;
    let server_value = serde_json::to_value(&server_spec)?;
    merge_asyncapi(&mut spec_value, &server_value);
    slim_component_messages(&mut spec_value)?;
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

/// Replace each component message's payload with just its variant schema and
/// hoist the shared `$defs` into `components.schemas`. Mutates `spec` in place.
#[expect(
    clippy::single_call_fn,
    reason = "named helper called once by build_spec(); the slim-down is one logical step"
)]
fn slim_component_messages(spec: &mut serde_json::Value) -> Result<(), Error> {
    use serde_json::Value;

    // Pluck one variant payload off any message — they're all identical, so
    // we use the first to harvest the `oneOf` array and `$defs`.
    let components = spec
        .get_mut("components")
        .and_then(Value::as_object_mut)
        .ok_or(Error::SpecNodeMissing("components"))?;

    let messages = components
        .get_mut("messages")
        .and_then(Value::as_object_mut)
        .ok_or(Error::SpecNodeMissing("components.messages"))?;

    let any_payload = messages
        .values()
        .find_map(|msg| msg.get("payload").cloned())
        .ok_or(Error::SpecNodeMissing("any message payload"))?;
    let one_of = any_payload
        .get("oneOf")
        .and_then(Value::as_array)
        .ok_or(Error::SpecNodeMissing("payload.oneOf"))?
        .clone();
    let defs = any_payload
        .get("$defs")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    // Index variants by their `type.const` discriminator so we can match each
    // component message name (`et-connect`, …) to its slim schema.
    let mut variants_by_tag: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
    for variant in one_of {
        let tag = variant
            .get("properties")
            .and_then(|props| props.get("type"))
            .and_then(|kind| kind.get("const"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let Some(tag) = tag else {
            continue;
        };
        // Rewrite `$ref: "#/$defs/Foo"` → `"#/components/schemas/Foo"` so the
        // hoisted defs land in the AsyncAPI-canonical location.
        let mut variant = variant;
        rewrite_refs(&mut variant);
        let _previous: Option<Value> = variants_by_tag.insert(tag, variant);
    }

    for (name, message) in messages.iter_mut() {
        if let Some(variant) = variants_by_tag.get(name)
            && let Some(obj) = message.as_object_mut()
        {
            let _previous: Option<Value> = obj.insert("payload".to_string(), variant.clone());
        }
    }

    // Hoist `$defs` to `components.schemas`. Rewrite refs inside each def too.
    let mut hoisted = serde_json::Map::new();
    for (name, mut value) in defs {
        rewrite_refs(&mut value);
        let _previous: Option<Value> = hoisted.insert(name, value);
    }
    if !hoisted.is_empty() {
        let _previous: Option<Value> = components.insert("schemas".to_string(), Value::Object(hoisted));
    }
    Ok(())
}

/// Recursively replace `$ref: "#/$defs/Foo"` with `"#/components/schemas/Foo"`.
fn rewrite_refs(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(reference) = map.get_mut("$ref")
                && let Some(raw) = reference.as_str()
                && let Some(rest) = raw.strip_prefix("#/$defs/")
            {
                *reference = serde_json::Value::String(format!("#/components/schemas/{rest}"));
            }
            for inner in map.values_mut() {
                rewrite_refs(inner);
            }
        }
        serde_json::Value::Array(items) => {
            for inner in items {
                rewrite_refs(inner);
            }
        }
        // primitives have no refs to rewrite.
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => {}
    }
}
