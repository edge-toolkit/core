// `Error` ends up ~272 bytes because `ureq::Error` is large and we wrap it
// inline via `#[from]`. Boxing it would shave the parent enum down but
// requires a manual `From<ureq::Error>` impl, since `#[from]` only generates
// `From<Box<ureq::Error>>`. For a one-shot CLI the size doesn't matter.
#![expect(
    clippy::result_large_err,
    reason = "Error inherits ureq::Error's byte footprint; immaterial for a one-shot CLI"
)]

//! Internal repo-only generator (`int` = internal).
//!
//! Emits every checked-in artifact under `generated/` from the Rust sources of
//! truth: `edge_toolkit::ws::WsMessage` for the WS protocol, and the
//! `#[utoipa::path]`-annotated handlers in `services/*` for the REST surface.
//! Driven by `mise run gen-specs`; `mise run gen-specs-check` fails if the
//! regenerated tree drifts from what's committed.
//!
//! Outputs (see [`generate`]):
//!   - `generated/specs/ws.yaml` — `AsyncAPI` 3.0 description of
//!     the WS protocol.
//!   - `generated/specs/rest.yaml` — `OpenAPI` 3.0 description of
//!     the ws-server's REST surface.
//!   - `generated/specs/wit/deps/et-ws-messages/messages.wit` — the typed
//!     WIT mirror of `WsMessage` consumed by `services/ws-wasi-runner` and
//!     every WASI ws-module. The accompanying top-level
//!     `generated/specs/wit/world.wit` is hand-maintained, not generated;
//!     see `generated/README.md`.
//!   - `generated/rust-rest/src/lib.rs` — typed Rust client for the REST
//!     surface, produced via `progenitor::Generator` from the `OpenAPI` doc.
//!   - `generated/dart-ws/lib/ws_messages.dart` — plain Dart 3 sealed classes.
//!     Pipeline: JSON Schema → KDL (this crate's [`kdl`] module) →
//!     `dart-typegen` CLI (driven by `mise run gen-dart-ws`).
//!   - `generated/python-ws/et_ws/messages.py` — Pydantic v2 models, written
//!     by `datamodel-codegen` (driven by `mise run gen-python-ws`).
//!   - `target/int-gen/ws.schema.json`,
//!     `target/int-gen/ws.kdl` — build intermediates (not
//!     committed); JSON Schema is the input to `datamodel-codegen`; KDL
//!     is the input to `dart-typegen`.
//!
//! Hand-maintained metadata that lives under `generated/` for proximity to
//! the generated code (package descriptions, dependency declarations) is
//! catalogued in `generated/README.md`. Upstream WASI WIT packages under
//! `generated/specs/wit/deps/wasi-*/` are pulled via
//! `mise run fetch-wit-deps`, handled by the companion [`wit::upstream`]
//! module.

use std::path::Path;

use asyncapi_rust::AsyncApi;
use edge_toolkit::config::get_project_root;
use edge_toolkit::ws::WsMessage;
use fs_err as fs;
use schemars::schema_for;

pub mod kdl;
pub mod rest;
pub mod wit;
pub mod zig;

/// Errors raised by `et-int-gen`.
///
/// Every external error type that fallible functions can produce is wrapped
/// transparently via `#[from]`, so call sites just use `?`. Domain errors
/// (malformed schemas, missing `AsyncAPI` nodes, etc.) sit alongside as
/// non-transparent variants with static messages.
#[expect(
    clippy::large_enum_variant,
    reason = "ureq::Error dominates the footprint; boxing it would force a manual From impl for no benefit in a CLI"
)]
#[expect(
    clippy::exhaustive_enums,
    reason = "et-int-gen is internal; no SemVer guarantee, new variants land alongside their introducing change"
)]
#[expect(
    clippy::error_impl_error,
    reason = "the crate's only error type lives at crate::Error, matching the rest of the workspace"
)]
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Yaml(#[from] serde_yaml::Error),
    #[error(transparent)]
    Http(#[from] ureq::Error),
    #[error(transparent)]
    Semver(#[from] semver::Error),
    #[error(transparent)]
    Fmt(#[from] std::fmt::Error),

    #[error("AsyncAPI spec missing required node: {0}")]
    SpecNodeMissing(&'static str),
    #[error("WsMessage JSON Schema malformed: {0}")]
    SchemaMalformed(&'static str),
    #[error("unsupported JSON Schema `type`: `{0}`")]
    UnsupportedSchemaType(String),
    #[error("enum value not a string in `{0}`")]
    EnumValueNotString(String),
    #[error("progenitor codegen: {0}")]
    Progenitor(String),
    #[error("zig codegen: {0}")]
    ZigCodegen(String),
}

impl From<progenitor::Error> for Error {
    fn from(err: progenitor::Error) -> Self {
        Self::Progenitor(err.to_string())
    }
}

impl From<tree_sitter::LanguageError> for Error {
    fn from(err: tree_sitter::LanguageError) -> Self {
        Self::ZigCodegen(format!("set Zig language: {err}"))
    }
}

/// `AsyncAPI` document for the ws-server's single `/ws` hub channel.
///
/// The `#[asyncapi_messages(WsMessage)]` attribute pulls every `WsMessage`
/// variant into `components.messages` automatically via the
/// `ToAsyncApiMessage` impl on the enum.
#[expect(
    clippy::duplicated_attributes,
    reason = "two #[asyncapi_operation(...)] entries are intentional (send/receive); collapsing drops a channel"
)]
#[derive(AsyncApi)]
#[asyncapi(
    title = "Edge Toolkit WebSocket Protocol",
    version = "0.1.0",
    description = "Hub-style WebSocket protocol. Generated from edge_toolkit::ws::WsMessage."
)]
#[asyncapi_server(
    name = "local",
    host = "localhost:8080",
    protocol = "ws",
    description = "Default ws-server bind address (mise run ws-server)"
)]
#[asyncapi_channel(name = "ws", address = "/ws")]
#[asyncapi_operation(name = "sendWsMessage", action = "send", channel = "ws")]
#[asyncapi_operation(name = "receiveWsMessage", action = "receive", channel = "ws")]
#[asyncapi_messages(WsMessage)]
struct WsApi;

/// Emit every checked-in artifact under `generated/` from the `WsMessage`
/// definition: `AsyncAPI` YAML, `et:ws-messages` WIT, Dart client, and the
/// intermediate JSON Schema under `target/int-gen/`.
#[expect(
    clippy::expect_used,
    clippy::unwrap_in_result,
    reason = "pretty_yaml::format_text only fails on a YAML syntax error; serde_yaml output is well-formed"
)]
#[expect(
    clippy::print_stderr,
    reason = "et-int-gen is a CLI; the skip notice when openapi2zig is absent is intentionally user-visible on stderr"
)]
pub fn generate() -> Result<(), Error> {
    let project_root = get_project_root();
    let specs_dir = project_root.join("generated/specs");

    // asyncapi-rust 0.2 fills every component message with the whole
    // `schema_for!(WsMessage)` payload (i.e. the full union), turning 13
    // 30-line messages into 13 220-line ones. We slim it down ourselves:
    // hoist `$defs` into `components.schemas` and give each message just
    // its matching `oneOf` variant.
    let spec = WsApi::asyncapi_spec();
    let mut spec_value = serde_json::to_value(&spec)?;
    slim_component_messages(&mut spec_value)?;
    // serde_yaml's emitter quotes/indents differently than dprint's
    // `pretty_yaml` plugin — pipe the output through `pretty_yaml` (the same
    // engine dprint uses) so the committed YAML stays dprint-canonical and
    // `dprint check` doesn't drift between regenerations.
    let yaml = serde_yaml::to_string(&spec_value)?;
    // serde_yaml always emits well-formed YAML, so pretty_yaml's parse step
    // can't fail here — the only error variant is a syntax error.
    let yaml = pretty_yaml::format_text(&yaml, &pretty_yaml::config::FormatOptions::default())
        .expect("serde_yaml output should always be well-formed");
    write_if_changed(&specs_dir.join("ws.yaml"), &yaml)?;

    // REST OpenAPI doc — emitted from utoipa annotations on actual handlers.
    let rest_yaml = rest::render_yaml();
    let rest_yaml = pretty_yaml::format_text(&rest_yaml, &pretty_yaml::config::FormatOptions::default())
        .expect("utoipa output should always be well-formed YAML");
    write_if_changed(&specs_dir.join("rest.yaml"), &rest_yaml)?;

    // Typed Rust client for the REST surface — same `progenitor::Generator`
    // engine that the retired `cargo-progenitor` CLI used, but driven
    // in-process so the spec and client always reflect the same source.
    let rust_client = rest::render_rust_client()?;
    write_if_changed(&project_root.join("generated/rust-rest/src/lib.rs"), &rust_client)?;

    // Zig client: openapi2zig generates a fully typed client, et-int-gen
    // post-processes it via tree-sitter-zig to swap the native HTTP
    // transport for an extern JS-fetch import (browser wasm target).

    // Upstream openapi2zig has no linux/arm64 release artifact, so mise
    // doesn't install it there — skip the whole step when the binary is
    // absent. The committed `generated/zig-rest/src/et_rest_client.zig`
    // stays untouched, so `gen-specs-check`'s `git diff --exit-code`
    // still passes on that host.
    if zig::is_available() {
        let rest_json_path = project_root.join("target/int-gen/rest.json");
        write_if_changed(&rest_json_path, &rest::render_json())?;
        let raw_zig_path = project_root.join("target/int-gen/raw_et_rest_client.zig");
        let zig_client = zig::render(&rest_json_path, &raw_zig_path)?;
        write_if_changed(
            &project_root.join("generated/zig-rest/src/et_rest_client.zig"),
            &zig_client,
        )?;
    } else {
        eprintln!("openapi2zig not found on PATH; skipping Zig REST client generation");
    }

    // Build intermediates land in target/ — datamodel-codegen reads the JSON
    // Schema for Python output, and dart-typegen reads the KDL for Dart.
    let schema = schema_for!(WsMessage);
    let schema_json = serde_json::to_string_pretty(&schema)?;
    let schema_path = project_root.join("target/int-gen/ws.schema.json");
    write_if_changed(&schema_path, &format!("{schema_json}\n"))?;

    let kdl_source = kdl::render(&schema)?;
    let kdl_path = project_root.join("target/int-gen/ws.kdl");
    write_if_changed(&kdl_path, &kdl_source)?;

    // The runner and all WASI guest crates point wit-bindgen / componentize-py
    // at `generated/specs/wit/` directly; the layout (main world at the top,
    // dep packages under `deps/`) follows the canonical wit-deps convention.
    // Only `deps/et-ws-messages/messages.wit` is generated (from the
    // `WsMessage` schema). The top-level `world.wit` is hand-maintained —
    // see `generated/README.md`.
    let wit_dir = project_root.join("generated/specs/wit");
    write_if_changed(
        &wit_dir.join("deps/et-ws-messages/messages.wit"),
        &wit::messages::render(&schema)?,
    )?;

    Ok(())
}

/// Replace each component message's payload with just its variant schema and
/// hoist the shared `$defs` into `components.schemas`. Mutates `spec` in place.
#[expect(
    clippy::single_call_fn,
    reason = "named helper called once by generate(); the slim-down is one logical step and benefits from its own scope"
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

/// Write only when the contents differ — keeps `mise run check` quiet on
/// no-op regenerations.
#[expect(
    clippy::print_stdout,
    reason = "et-int-gen is a CLI; `wrote <path>` per generated file is intended user-visible progress output"
)]
pub(crate) fn write_if_changed(path: &Path, contents: &str) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let unchanged = fs::read_to_string(path).is_ok_and(|existing| existing == contents);
    if unchanged {
        return Ok(());
    }
    fs::write(path, contents)?;
    println!("wrote {}", path.display());
    Ok(())
}
