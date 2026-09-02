//! Internal repo-only generator (`int` = internal).
//!
//! Emits every checked-in artifact under `generated/` from the Rust sources of
//! truth: `edge_toolkit::ws::{ClientMessage, ServerMessage}` for the WS protocol, and the
//! `#[utoipa::path]`-annotated handlers in `services/*` for the REST surface.
//! Driven by `mise run gen-specs`; `mise run gen-specs-check` fails if the
//! regenerated tree drifts from what's committed.
//!
//! Outputs (see [`generate`]):
//!   - `generated/specs/ws.yaml` -- `AsyncAPI` 3.0 description of
//!     the WS protocol.
//!   - `generated/specs/rest.yaml` -- `OpenAPI` 3.0 description of
//!     the ws-server's REST surface.
//!   - `generated/specs/wit/deps/et-ws-messages/messages.wit` -- the typed
//!     WIT mirror of the two message enums consumed by `services/ws-wasi-runner` and
//!     every WASI ws-module. The accompanying top-level
//!     `generated/specs/wit/world.wit` is hand-maintained, not generated;
//!     see `generated/README.md`.
//!   - `generated/rust-rest/src/lib.rs` -- typed Rust client for the REST
//!     surface, produced via `progenitor::Generator` from the `OpenAPI` doc.
//!   - `generated/dart-ws/lib/ws_messages.dart` -- plain Dart 3 sealed classes.
//!     Pipeline: JSON Schema -> KDL (this crate's [`kdl`] module) ->
//!     `dart-typegen` CLI (driven by `mise run gen:dart-ws`).
//!   - `generated/python-ws/et_ws/messages.py` -- Pydantic v2 models, written
//!     by `datamodel-codegen` (driven by `mise run gen:python-ws`).
//!   - `generated/specs/ws.kdl` -- checked-in KDL projection of the WS
//!     schema; the input `dart-typegen` reads to produce the Dart client.
//!   - `target/int-gen/ws.schema.json` -- build intermediate (not
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
pub mod error;
pub mod kdl;
pub mod openapi;
pub mod wit;
pub mod zig;

pub use self::error::Error;

/// Emit every checked-in artifact under `generated/` (core + Rust + Zig).
///
/// Convenience wrapper over the three per-target functions; `mise run
/// gen-specs` drives them individually per `MISE_ENV` instead.
pub fn generate() -> Result<(), Error> {
    generate_core()?;
    generate_rust()?;
    generate_bindings()?;
    generate_zig()?;
    Ok(())
}

/// Emit the wasmtime host bindings for the `runner` world.
///
/// Writes `services/ws-wasi-runner/src/bindings.rs`, which used to be expanded from
/// `wasmtime::component::bindgen!` at build time against a WIT path outside that crate -- a path
/// `cargo package` cannot include, leaving the crate unable to build from its own tarball. Depends on the
/// `et:ws-messages` WIT that [`generate_core`] emits, so run it after that.
pub fn generate_bindings() -> Result<(), Error> {
    let project_root = get_project_root();
    let rendered = wit::bindings::render(&project_root.join("generated/specs/wit"))?;
    write_if_changed(&project_root.join("services/ws-wasi-runner/src/bindings.rs"), &rendered)?;
    Ok(())
}

/// Emit the language-agnostic artifacts.
///
/// Namely `ws.yaml`, `rest.yaml`, the `ws.schema.json` intermediates, `ws.kdl`, and the `et:ws-messages` WIT
/// package. These feed every downstream client (the Dart/Python generators consume
/// `ws.kdl`/`*.schema.json`/`rest.yaml`), so this is the prerequisite step
/// every per-language `gen:*` mise task depends on.
#[expect(
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    reason = "pretty_yaml only fails on malformed YAML and serde output is always well-formed"
)]
pub fn generate_core() -> Result<(), Error> {
    let project_root = get_project_root();
    let specs_dir = project_root.join("generated/specs");

    // Build, merge, and slim the AsyncAPI spec -- all the spec-shaping
    // logic lives in `asyncapi`. The returned `Value` is what we serialise
    // to ws.yaml below.
    let spec_value = asyncapi::build_spec()?;
    // serde_yaml's emitter quotes/indents differently than dprint's
    // `pretty_yaml` plugin -- pipe the output through `pretty_yaml` (the same
    // engine dprint uses) so the committed YAML stays dprint-canonical and
    // `dprint check` doesn't drift between regenerations.
    let yaml = serde_yaml::to_string(&spec_value)?;
    // serde_yaml always emits well-formed YAML, so pretty_yaml's parse step
    // can't fail here -- the only error variant is a syntax error.
    let yaml = pretty_yaml::format_text(&yaml, &pretty_yaml::config::FormatOptions::default()).unwrap();
    write_if_changed(&specs_dir.join("ws.yaml"), &yaml)?;

    // REST OpenAPI doc -- emitted from utoipa annotations on actual handlers.
    let rest_yaml = openapi::render_yaml();
    let rest_yaml = pretty_yaml::format_text(&rest_yaml, &pretty_yaml::config::FormatOptions::default()).unwrap();
    write_if_changed(&specs_dir.join("rest.yaml"), &rest_yaml)?;

    // Build intermediates land in target/ -- datamodel-codegen reads the JSON
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

/// Emit the typed Rust REST client (`generated/rust-rest/src/lib.rs`).
///
/// Same `progenitor::Generator` engine the retired `cargo-progenitor` CLI
/// used, but driven in-process so the spec and client always reflect the
/// same source. Self-contained: re-derives the `OpenAPI` doc in-process, so
/// it needs no ordering relative to [`generate_core`].
pub fn generate_rust() -> Result<(), Error> {
    let project_root = get_project_root();
    let rust_client = openapi::render_rust_client()?;
    write_if_changed(&project_root.join("generated/rust-rest/src/lib.rs"), &rust_client)?;
    Ok(())
}

/// Emit the Zig REST client (`generated/zig-rest/src/et_rest_client.zig`).
///
/// openapi2zig generates a fully typed client; et-int-gen post-processes it
/// via tree-sitter-zig to swap the native HTTP transport for an extern
/// JS-fetch import (browser wasm target). Self-contained: re-derives the
/// `OpenAPI` JSON in-process.
///
/// Upstream openapi2zig has no linux/arm64 release artifact, so mise doesn't
/// install it there -- skip the whole step when the binary is absent. The
/// committed `generated/zig-rest/src/et_rest_client.zig` stays untouched, so
/// `gen-specs-check`'s `git diff --exit-code` still passes on that host.
#[expect(
    clippy::print_stderr,
    reason = "et-int-gen is a CLI; the skip notice on stderr is intended user-visible output"
)]
pub fn generate_zig() -> Result<(), Error> {
    let project_root = get_project_root();
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
    Ok(())
}

/// Write only when the contents differ, to keep `mise run check` quiet on no-op regenerations.
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
