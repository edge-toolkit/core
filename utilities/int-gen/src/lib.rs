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
//! truth: `edge_toolkit::ws::{ClientMessage, ServerMessage}` for the WS protocol, and the
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
//!     WIT mirror of the two message enums consumed by `services/ws-wasi-runner` and
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
//!   - `generated/specs/ws.kdl` — checked-in KDL projection of the WS
//!     schema; the input `dart-typegen` reads to produce the Dart client.
//!   - `target/int-gen/ws.schema.json` — build intermediate (not
//!     committed); the input `datamodel-codegen` reads for the Python
//!     client.
//!
//! Hand-maintained metadata that lives under `generated/` for proximity to
//! the generated code (package descriptions, dependency declarations) is
//! catalogued in `generated/README.md`. Upstream WASI WIT packages under
//! `generated/specs/wit/deps/wasi-*/` are pulled via
//! `mise run fetch-wit-deps`, handled by the companion [`wit::upstream`]
//! module.

use std::path::Path;

use edge_toolkit::config::get_project_root;
use edge_toolkit::ws::{ClientMessage, ServerMessage};
use fs_err as fs;
use schemars::schema_for;

pub mod asyncapi;
pub mod kdl;
pub mod openapi;
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
    #[error("WS message JSON Schema malformed: {0}")]
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

/// Emit every checked-in artifact under `generated/`.
///
/// Inputs: the `ClientMessage` + `ServerMessage` enums; outputs are the
/// `AsyncAPI` YAML, the `et:ws-messages` WIT package, the Dart client,
/// and the intermediate JSON Schemas under `target/int-gen/`.
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

    // Build, merge, and slim the AsyncAPI spec — all the spec-shaping
    // logic lives in `asyncapi`. The returned `Value` is what we serialise
    // to ws.yaml below.
    let spec_value = asyncapi::build_spec()?;
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
    let rest_yaml = openapi::render_yaml();
    let rest_yaml = pretty_yaml::format_text(&rest_yaml, &pretty_yaml::config::FormatOptions::default())
        .expect("utoipa output should always be well-formed YAML");
    write_if_changed(&specs_dir.join("rest.yaml"), &rest_yaml)?;

    // Typed Rust client for the REST surface — same `progenitor::Generator`
    // engine that the retired `cargo-progenitor` CLI used, but driven
    // in-process so the spec and client always reflect the same source.
    let rust_client = openapi::render_rust_client()?;
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
        write_if_changed(&rest_json_path, &openapi::render_json())?;
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
    // Both halves of the protocol contribute schema; the KDL + WIT
    // generators consume them as `(client, server)` pairs.
    let client_schema = schema_for!(ClientMessage);
    let server_schema = schema_for!(ServerMessage);
    let schema_json = serde_json::to_string_pretty(&client_schema)?;
    let schema_path = project_root.join("target/int-gen/ws.schema.json");
    write_if_changed(&schema_path, &format!("{schema_json}\n"))?;
    let server_schema_path = project_root.join("target/int-gen/ws.server.schema.json");
    write_if_changed(
        &server_schema_path,
        &format!("{}\n", serde_json::to_string_pretty(&server_schema)?),
    )?;

    let kdl_source = kdl::render(&client_schema, &server_schema)?;
    let kdl_path = project_root.join("generated/specs/ws.kdl");
    write_if_changed(&kdl_path, &kdl_source)?;

    let wit_dir = project_root.join("generated/specs/wit");
    write_if_changed(
        &wit_dir.join("deps/et-ws-messages/messages.wit"),
        &wit::messages::render(&client_schema, &server_schema)?,
    )?;

    Ok(())
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
