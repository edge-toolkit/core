//! Emit a `dart-typegen`-flavoured KDL document from the `ClientMessage`
//! and `ServerMessage` JSON Schemas.
//!
//! The KDL goes to `generated/specs/ws.kdl` (checked in alongside
//! `ws.yaml`); `dart-typegen generate -i generated/specs/ws.kdl -o ...`
//! consumes it to produce `generated/dart-ws/lib/ws_messages.dart`.
//!
//! Why this layer exists: dart-typegen consumes KDL declaratively (classes,
//! enums, unions with `json-discriminant`), so we only have to bridge from
//! schemars' JSON Schema shape to that vocabulary. The hand-rolled Dart
//! emitter previously lived in `dart.rs`.

use heck::{ToLowerCamelCase as _, ToPascalCase as _};
use kdl::{KdlDocument, KdlEntry, KdlEntryFormat, KdlIdentifier, KdlNode, KdlValue};
use schemars::Schema;

use crate::Error;

/// `dart-typegen` parses KDL v1 via knus 3.x, which is stricter than the
/// shipping KDL v1/v2 specs: identifier-shaped strings (`String`, `AgentSummary`)
/// must always appear quoted. The kdl crate's auto-formatter happily drops
/// those quotes, so every entry we emit explicitly pins its `value_repr` and
/// flips `autoformat_keep` to prevent that.
fn quoted_string_entry(raw: &str) -> KdlEntry {
    let mut entry = KdlEntry::new(KdlValue::String(raw.into()));
    let mut format = KdlEntryFormat::default();
    format.value_repr = format!("\"{raw}\"");
    format.leading = " ".to_string();
    format.autoformat_keep = true;
    entry.set_format(format);
    entry
}

#[expect(
    clippy::single_call_fn,
    reason = "paired with quoted_string_entry; centralises the KdlEntryFormat dance knus 3.x needs"
)]
fn quoted_string_prop(key: &str, value: &str) -> KdlEntry {
    let mut entry = KdlEntry::new_prop(KdlIdentifier::from(key), KdlValue::String(value.into()));
    let mut format = KdlEntryFormat::default();
    format.value_repr = format!("\"{value}\"");
    format.leading = " ".to_string();
    format.autoformat_keep = true;
    entry.set_format(format);
    entry
}

pub fn render(client_schema: &Schema, server_schema: &Schema) -> Result<String, Error> {
    let client_root = client_schema.as_value();
    let server_root = server_schema.as_value();
    let mut doc = KdlDocument::new();
    doc.nodes_mut().push(defaults_node());

    // Merge `$defs` from both schemas, deduplicated by name. The two
    // enums share most support types (AgentSummary, ConnectStatus, …);
    // we emit each definition exactly once.
    let mut merged_defs: std::collections::BTreeMap<String, serde_json::Value> = std::collections::BTreeMap::new();
    for root in [client_root, server_root] {
        if let Some(defs) = root.get("$defs").and_then(|val| val.as_object()) {
            for (name, def) in defs {
                let _previous = merged_defs.insert(name.clone(), def.clone());
            }
        }
    }
    for (name, def) in &merged_defs {
        if def.get("enum").is_some() {
            doc.nodes_mut().push(enum_node(name, def)?);
        }
    }
    for (name, def) in &merged_defs {
        if def.get("enum").is_none() && def.get("type").and_then(|kind| kind.as_str()) == Some("object") {
            doc.nodes_mut().push(class_node(name, def, None)?);
        }
    }

    // Compute which variant tags appear in BOTH unions; those payload
    // classes get a per-direction prefix (`WsClientRelayText` vs
    // `WsServerRelayText`) so dart-typegen sees them as distinct
    // declarations. Variants unique to a single union keep the bare
    // `Ws<Pascal>` name.
    let shared_tags: std::collections::HashSet<String> = variant_tags(client_root)?
        .intersection(&variant_tags(server_root)?)
        .cloned()
        .collect();
    doc.nodes_mut()
        .push(message_union(client_root, "WsClientMessage", "WsClient", &shared_tags)?);
    doc.nodes_mut()
        .push(message_union(server_root, "WsServerMessage", "WsServer", &shared_tags)?);
    doc.autoformat();
    // `dart-typegen` parses KDL v1 (via knus); the kdl crate emits v2 syntax
    // by default (`#true`/`#null`). Force v1 so booleans and null render as
    // bare `true`/`null` tokens that knus understands.
    doc.ensure_v1();
    Ok(format!("{doc}"))
}

/// The `defaults` block tells dart-typegen to emit sealed unions keyed on
/// `"type"` and to convert `camelCase` Dart field names to `snake_case` JSON
/// keys — matching how the Rust serde tag/`rename_all` configuration writes
/// the wire.
#[expect(
    clippy::single_call_fn,
    reason = "named helper for the top-level `defaults` block; kept separate to scope its nested children"
)]
fn defaults_node() -> KdlNode {
    let mut defaults = KdlNode::new("defaults");
    let mut children = KdlDocument::new();

    let mut union_defaults = KdlNode::new("union");
    let mut union_children = KdlDocument::new();
    let mut sealed = KdlNode::new("sealed");
    sealed.push(KdlValue::Bool(true));
    union_children.nodes_mut().push(sealed);
    let mut discriminant = KdlNode::new("json-discriminant");
    discriminant.push(quoted_string_entry("type"));
    union_children.nodes_mut().push(discriminant);
    union_defaults.set_children(union_children);
    children.nodes_mut().push(union_defaults);

    let mut field_defaults = KdlNode::new("field");
    let mut field_children = KdlDocument::new();
    let mut key_case = KdlNode::new("json-key-case");
    key_case.push(quoted_string_entry("snake"));
    field_children.nodes_mut().push(key_case);
    field_defaults.set_children(field_children);
    children.nodes_mut().push(field_defaults);

    defaults.set_children(children);
    defaults
}

#[expect(
    clippy::single_call_fn,
    reason = "named helper called once by render(); split keeps the enum-emitter near its siblings"
)]
fn enum_node(name: &str, def: &serde_json::Value) -> Result<KdlNode, Error> {
    let values = def
        .get("enum")
        .and_then(|val| val.as_array())
        .ok_or(Error::SchemaMalformed("enum def missing `enum` array"))?;
    let mut node = KdlNode::new("enum");
    node.push(quoted_string_entry(name));
    let mut children = KdlDocument::new();
    for value in values {
        let raw = value
            .as_str()
            .ok_or_else(|| Error::EnumValueNotString(name.to_string()))?;
        let mut variant = KdlNode::new("variant");
        variant.push(quoted_string_entry(raw));
        children.nodes_mut().push(variant);
    }
    node.set_children(children);
    Ok(node)
}

/// `discriminator` is set when the class belongs to a union — dart-typegen
/// then emits `json-discriminant-value "et-..."` inside the class body.
fn class_node(name: &str, schema: &serde_json::Value, discriminator: Option<&str>) -> Result<KdlNode, Error> {
    let props = schema.get("properties").and_then(|val| val.as_object());
    let required: std::collections::HashSet<&str> = schema
        .get("required")
        .and_then(|val| val.as_array())
        .map(|arr| arr.iter().filter_map(|val| val.as_str()).collect())
        .unwrap_or_default();

    let mut node = KdlNode::new("class");
    node.push(quoted_string_entry(name));

    let mut children = KdlDocument::new();
    if let Some(tag) = discriminator {
        let mut tag_node = KdlNode::new("json-discriminant-value");
        tag_node.push(quoted_string_entry(tag));
        children.nodes_mut().push(tag_node);
    }
    if let Some(props) = props {
        let mut keys: Vec<&String> = props.keys().filter(|key| key.as_str() != "type").collect();
        keys.sort();
        for key in keys {
            let prop_schema = &props[key];
            let optional = !required.contains(key.as_str());
            children.nodes_mut().push(field_node(key, prop_schema, optional)?);
        }
    }
    if !children.is_empty() {
        node.set_children(children);
    }
    Ok(node)
}

#[expect(
    clippy::single_call_fn,
    reason = "named helper called once by class_node(); kept separate to scope the field-emission rules"
)]
fn field_node(key: &str, schema: &serde_json::Value, optional: bool) -> Result<KdlNode, Error> {
    let mut node = KdlNode::new("field");
    node.push(quoted_string_entry(&key.to_lower_camel_case()));
    let dart_type = dart_type_from(schema, optional)?;
    node.push(quoted_string_prop("type", &dart_type));
    if optional || dart_type.ends_with('?') {
        let mut children = KdlDocument::new();
        let mut default = KdlNode::new("defaults-to");
        default.push(KdlValue::Null);
        children.nodes_mut().push(default);
        node.set_children(children);
    }
    Ok(node)
}

fn message_union(
    root: &serde_json::Value,
    union_name: &str,
    shared_prefix: &str,
    shared_tags: &std::collections::HashSet<String>,
) -> Result<KdlNode, Error> {
    let variants = root
        .get("oneOf")
        .and_then(|val| val.as_array())
        .ok_or(Error::SchemaMalformed("message schema missing `oneOf`"))?;

    let mut node = KdlNode::new("union");
    node.push(quoted_string_entry(union_name));
    let mut children = KdlDocument::new();
    for variant in variants {
        let tag = variant_tag(variant)?;
        let suffix = tag.strip_prefix("et-").unwrap_or(&tag).to_pascal_case();
        let class_name = if shared_tags.contains(&tag) {
            format!("{shared_prefix}{suffix}")
        } else {
            format!("Ws{suffix}")
        };
        children.nodes_mut().push(class_node(&class_name, variant, Some(&tag))?);
    }
    node.set_children(children);
    Ok(node)
}

/// Collect every variant's `type` discriminator string from a top-level
/// `oneOf` schema. Used to identify variants that appear in both the
/// client and server unions so their payload classes can be uniquely
/// named.
fn variant_tags(root: &serde_json::Value) -> Result<std::collections::HashSet<String>, Error> {
    let variants = root
        .get("oneOf")
        .and_then(|val| val.as_array())
        .ok_or(Error::SchemaMalformed("message schema missing `oneOf`"))?;
    variants.iter().map(variant_tag).collect()
}

fn variant_tag(variant: &serde_json::Value) -> Result<String, Error> {
    variant
        .get("properties")
        .and_then(|props| props.get("type"))
        .and_then(|kind| kind.get("const"))
        .and_then(|cnst| cnst.as_str())
        .map(str::to_string)
        .ok_or(Error::SchemaMalformed("variant missing const `type` discriminator"))
}

/// JSON Schema → Dart type expression understood by `dart-typegen`.
///
/// Mirrors the matching helper in the WIT emitter, but spells primitives the
/// Dart way (`String`, `int`, …) and uses `List<T>` / `Map<String, dynamic>`
/// for collection / opaque-JSON shapes.
fn dart_type_from(schema: &serde_json::Value, force_optional: bool) -> Result<String, Error> {
    if let Some(reference) = schema.get("$ref").and_then(|val| val.as_str()) {
        let name = reference
            .rsplit('/')
            .next()
            .ok_or(Error::SchemaMalformed("malformed $ref"))?;
        return Ok(append_q(name, force_optional));
    }
    if let Some(any_of) = schema.get("anyOf").and_then(|val| val.as_array()) {
        let non_null: Vec<&serde_json::Value> = any_of
            .iter()
            .filter(|sch| sch.get("type").and_then(|kind| kind.as_str()) != Some("null"))
            .collect();
        if let [single] = non_null.as_slice() {
            let inner = dart_type_from(single, false)?;
            return Ok(append_q_force(&inner));
        }
    }
    if let Some(types) = schema.get("type").and_then(|val| val.as_array()) {
        let primary = types
            .iter()
            .find_map(|val| val.as_str().filter(|kind| *kind != "null"))
            .ok_or(Error::SchemaMalformed("type array had no non-null entry"))?;
        let nullable = types.iter().any(|val| val.as_str() == Some("null"));
        return Ok(append_q(&primitive(primary, schema)?, force_optional || nullable));
    }
    if let Some(kind) = schema.get("type").and_then(|val| val.as_str()) {
        return Ok(append_q(&primitive(kind, schema)?, force_optional));
    }
    Ok(append_q("Map<String, dynamic>", force_optional))
}

fn primitive(kind: &str, schema: &serde_json::Value) -> Result<String, Error> {
    Ok(match kind {
        "string" => "String".to_string(),
        "integer" => "int".to_string(),
        "number" => "double".to_string(),
        "boolean" => "bool".to_string(),
        "array" => {
            let items = schema
                .get("items")
                .ok_or(Error::SchemaMalformed("array schema missing items"))?;
            let inner = dart_type_from(items, false)?;
            format!("List<{inner}>")
        }
        "object" => "Map<String, dynamic>".to_string(),
        other => return Err(Error::UnsupportedSchemaType(other.to_string())),
    })
}

fn append_q(kind: &str, optional: bool) -> String {
    if optional {
        append_q_force(kind)
    } else {
        kind.to_string()
    }
}

fn append_q_force(kind: &str) -> String {
    if kind.ends_with('?') {
        kind.to_string()
    } else {
        format!("{kind}?")
    }
}
