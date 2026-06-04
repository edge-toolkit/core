//! Translates a `schemars` JSON Schema for `WsMessage` into the
//! `et:ws-messages@0.1.0` WIT package.
//!
//! Built with `wit-encoder` so the output format is canonical and the
//! construction is type-checked — we never produce manual `writeln!` lines.
//!
//! Mapping rules:
//!   * Variant rename `et-foo-bar` → variant case `foo-bar`. The `et-`
//!     prefix is dropped because the WIT package namespace (`et:`)
//!     already carries it.
//!   * `serde_json::Value` fields → `string` (the host serializes the
//!     opaque JSON when shipping to/from the guest).
//!   * `Option<T>` → `option<T>`. `Vec<T>` → `list<T>`. `String` →
//!     `string`. Integers → `s64` (the wire format never narrows).
//!   * `#[serde(rename_all = "snake_case")]` enums map directly to WIT
//!     `enum` with kebab-case case names.

use std::collections::HashSet;

use heck::ToKebabCase as _;
use schemars::Schema;
use wit_encoder::{EnumCase, Field, Ident, Interface, Package, PackageName, Type, TypeDef, VariantCase};

use crate::Error;

/// Wire identifiers (`WsConnectAck`, `agent_id`, `et-connect-ack`, …) →
/// canonical WIT kebab-case (`ws-connect-ack`, `agent-id`, `connect-ack`).
/// The `et-` prefix is dropped because the `et:ws-messages` WIT package
/// namespace already carries it.
fn to_kebab(input: &str) -> String {
    input.strip_prefix("et-").unwrap_or(input).to_kebab_case()
}

type EnumSet = HashSet<String>;

#[expect(
    clippy::expect_used,
    clippy::unwrap_in_result,
    reason = "the semver literal is a compile-time constant; an Err here means the literal itself was mistyped, which is a developer error caught by the next test run"
)]
pub fn render(root_schema: &Schema) -> Result<String, Error> {
    let root = root_schema.as_value();
    let mut interface = Interface::new("messages");
    interface.set_docs(Some(
        "Typed WS protocol messages \u{2014} each `ws-message` case maps 1:1 to a Rust `WsMessage` variant on the wire.",
    ));

    let enums = collect_enum_names(root);
    emit_enum_defs(root, &mut interface)?;
    emit_record_defs(root, &mut interface, &enums)?;
    emit_variant_payloads(root, &mut interface, &enums)?;
    emit_top_level_variant(root, &mut interface)?;

    let mut package = Package::new(PackageName::new(
        "et",
        "ws-messages",
        Some(semver::Version::parse("0.1.0").expect("valid semver")),
    ));
    package.interface(interface);

    Ok(package.to_string())
}

#[expect(
    clippy::single_call_fn,
    reason = "named helper called once by render(); pre-computes the enum-name set the other emitters consume"
)]
fn collect_enum_names(root: &serde_json::Value) -> EnumSet {
    root.get("$defs")
        .and_then(|val| val.as_object())
        .map(|defs| {
            defs.iter()
                .filter(|(_, def)| def.get("enum").is_some())
                .map(|(name, _)| name.clone())
                .collect()
        })
        .unwrap_or_default()
}

#[expect(
    clippy::single_call_fn,
    reason = "named helper called once by render(); kept separate so each emit_* phase has its own scope"
)]
fn emit_enum_defs(root: &serde_json::Value, interface: &mut Interface) -> Result<(), Error> {
    let Some(defs) = root.get("$defs").and_then(|val| val.as_object()) else {
        return Ok(());
    };
    let mut names: Vec<&String> = defs.keys().collect();
    names.sort();
    for name in names {
        let def = &defs[name];
        let Some(values) = def.get("enum").and_then(|val| val.as_array()) else {
            continue;
        };
        let cases: Vec<EnumCase> = values
            .iter()
            .map(|val| {
                val.as_str()
                    .ok_or_else(|| Error::EnumValueNotString(name.clone()))
                    .map(|raw| EnumCase::new(to_kebab(raw)))
            })
            .collect::<Result<_, Error>>()?;
        interface.type_def(TypeDef::enum_(to_kebab(name), cases));
    }
    Ok(())
}

#[expect(
    clippy::single_call_fn,
    reason = "named helper called once by render(); pairs with emit_enum_defs / emit_variant_payloads / emit_top_level_variant"
)]
fn emit_record_defs(root: &serde_json::Value, interface: &mut Interface, enums: &EnumSet) -> Result<(), Error> {
    let Some(defs) = root.get("$defs").and_then(|val| val.as_object()) else {
        return Ok(());
    };
    let mut names: Vec<&String> = defs.keys().collect();
    names.sort();
    for name in names {
        let def = &defs[name];
        if def.get("enum").is_some() || def.get("type").and_then(|kind| kind.as_str()) != Some("object") {
            continue;
        }
        interface.type_def(build_record(name, def, enums, false)?);
    }
    Ok(())
}

#[expect(
    clippy::single_call_fn,
    reason = "named helper called once by render(); pairs with emit_top_level_variant"
)]
fn emit_variant_payloads(root: &serde_json::Value, interface: &mut Interface, enums: &EnumSet) -> Result<(), Error> {
    let variants = root
        .get("oneOf")
        .and_then(|val| val.as_array())
        .ok_or(Error::SchemaMalformed("WsMessage schema missing `oneOf`"))?;
    for variant in variants {
        if !variant_has_payload(variant) {
            continue;
        }
        let tag = variant_tag(variant)?;
        let record_name = format!("{}-payload", to_kebab(&tag));
        interface.type_def(build_record(&record_name, variant, enums, true)?);
    }
    Ok(())
}

#[expect(
    clippy::single_call_fn,
    reason = "named helper called once by render(); emits the top-level tagged-union typedef"
)]
fn emit_top_level_variant(root: &serde_json::Value, interface: &mut Interface) -> Result<(), Error> {
    let variants = root
        .get("oneOf")
        .and_then(|val| val.as_array())
        .ok_or(Error::SchemaMalformed("WsMessage schema missing `oneOf`"))?;
    let cases: Vec<VariantCase> = variants
        .iter()
        .map(|variant| {
            let tag = variant_tag(variant)?;
            let case_name: Ident = to_kebab(&tag).into();
            if variant_has_payload(variant) {
                let payload_name: Ident = format!("{}-payload", to_kebab(&tag)).into();
                Ok(VariantCase::value(case_name, Type::Named(payload_name)))
            } else {
                Ok(VariantCase::empty(case_name))
            }
        })
        .collect::<Result<_, Error>>()?;
    let mut variant_def = TypeDef::variant("ws-message", cases);
    variant_def.set_docs(Some("Tagged union covering every wire-format WS message."));
    interface.type_def(variant_def);
    Ok(())
}

fn build_record(
    name: &str,
    schema: &serde_json::Value,
    enums: &EnumSet,
    skip_type_discriminator: bool,
) -> Result<TypeDef, Error> {
    let props = schema.get("properties").and_then(|val| val.as_object());
    let required: HashSet<&str> = schema
        .get("required")
        .and_then(|val| val.as_array())
        .map(|arr| arr.iter().filter_map(|val| val.as_str()).collect())
        .unwrap_or_default();
    let mut fields: Vec<Field> = Vec::new();
    if let Some(props) = props {
        let mut keys: Vec<&String> = props
            .keys()
            .filter(|key| !skip_type_discriminator || key.as_str() != "type")
            .collect();
        keys.sort();
        for key in keys {
            let prop_schema = &props[key];
            let optional = !required.contains(key.as_str());
            let field_ty = wit_type_from(prop_schema, optional, enums)?;
            fields.push(Field::new(to_kebab(key), field_ty));
        }
    }
    Ok(TypeDef::record(to_kebab(name), fields))
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

fn variant_has_payload(variant: &serde_json::Value) -> bool {
    variant
        .get("properties")
        .and_then(|val| val.as_object())
        .is_some_and(|props| props.keys().any(|key| key.as_str() != "type"))
}

fn wit_type_from(schema: &serde_json::Value, force_optional: bool, enums: &EnumSet) -> Result<Type, Error> {
    if let Some(reference) = schema.get("$ref").and_then(|val| val.as_str()) {
        let name = reference
            .rsplit('/')
            .next()
            .ok_or(Error::SchemaMalformed("malformed $ref"))?;
        // `enums` is tracked for future use (e.g. payload typing); not
        // needed on the `$ref` branch.
        return Ok(wrap_optional(Type::Named(to_kebab(name).into()), force_optional));
    }
    if let Some(any_of) = schema.get("anyOf").and_then(|val| val.as_array()) {
        let non_null: Vec<&serde_json::Value> = any_of
            .iter()
            .filter(|sch| sch.get("type").and_then(|kind| kind.as_str()) != Some("null"))
            .collect();
        if let [single] = non_null.as_slice() {
            let inner = wit_type_from(single, false, enums)?;
            return Ok(Type::option(inner));
        }
    }
    if let Some(types) = schema.get("type").and_then(|val| val.as_array()) {
        let primary = types
            .iter()
            .find_map(|val| val.as_str().filter(|kind| *kind != "null"))
            .ok_or(Error::SchemaMalformed("type array had no non-null entry"))?;
        let nullable = types.iter().any(|val| val.as_str() == Some("null"));
        let base = primitive(primary, schema, enums)?;
        return Ok(wrap_optional(base, force_optional || nullable));
    }
    if let Some(kind) = schema.get("type").and_then(|val| val.as_str()) {
        return Ok(wrap_optional(primitive(kind, schema, enums)?, force_optional));
    }
    // `serde_json::Value` fields hit this branch via the `any_json_schema`
    // hook — no `type` keyword, just a description. Ship them as opaque
    // JSON strings so the host can round-trip arbitrary payloads.
    Ok(wrap_optional(Type::String, force_optional))
}

fn primitive(kind: &str, schema: &serde_json::Value, enums: &EnumSet) -> Result<Type, Error> {
    Ok(match kind {
        // serde_json::Value-shaped opaque objects collapse onto `string`
        // alongside genuine strings — the host serialises them to JSON.
        "string" | "object" => Type::String,
        "integer" => Type::S64,
        "number" => Type::F64,
        "boolean" => Type::Bool,
        "array" => {
            let items = schema
                .get("items")
                .ok_or(Error::SchemaMalformed("array schema missing items"))?;
            let inner = wit_type_from(items, false, enums)?;
            Type::list(inner)
        }
        other => return Err(Error::UnsupportedSchemaType(other.to_string())),
    })
}

fn wrap_optional(inner: Type, optional: bool) -> Type {
    if optional { Type::option(inner) } else { inner }
}
