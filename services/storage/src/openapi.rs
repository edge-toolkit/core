//! `#[utoipa::path]`-annotated stubs for the storage routes.
//!
//! The real PUT handler lives in `routes.rs`; the GET request is served by
//! an `actix_files::Files` mount registered in `crate::configure`. The
//! functions here exist solely to host the `#[utoipa::path]` macro:
//! utoipa generates a sibling `__path_*` struct for each annotated
//! function, and that's what `int-gen` references through the `paths(...)`
//! list to build the spec.
//!
//! The file-level `#![expect(clippy::exhaustive_structs)]` (active under
//! `openapi-spec`) is scoped here because that lint can fire only against
//! the `__path_*` types — and they aren't reachable from an `#[expect]` on
//! the function itself.

#![cfg_attr(
    feature = "openapi-spec",
    expect(
        clippy::exhaustive_structs,
        reason = "utoipa::path emits `__path_*` types without #[non_exhaustive]; scoped only to this openapi module"
    )
)]

#[cfg(feature = "openapi-spec")]
use actix_web::HttpResponse;

#[cfg(feature = "openapi-spec")]
use crate::BinaryBlob;

/// Upload a file to the named agent's storage bucket. Only the agent that
/// owns the bucket may write to it (the agent must currently be
/// connected); the path component must be a single filename, not a nested
/// path.
#[cfg(feature = "openapi-spec")]
#[expect(
    dead_code,
    reason = "openapi placeholder: only the sibling `__path_put_file` type emitted by `#[utoipa::path]` is referenced"
)]
#[utoipa::path(
    put,
    path = "/storage/{agent_id}/{filename}",
    params(
        ("agent_id" = String, Path, description = "Agent identifier (must be a connected agent)"),
        ("filename" = String, Path, description = "Single-segment filename to write")
    ),
    request_body(
        content = inline(BinaryBlob),
        content_type = "application/octet-stream",
        description = "Raw file bytes"
    ),
    responses(
        (status = 200, description = "File stored"),
        (status = 400, description = "Invalid filename"),
        (status = 404, description = "Agent not found")
    )
)]
pub fn put_file() -> HttpResponse {
    HttpResponse::NotImplemented().finish()
}

/// Download a file previously written to the named agent's storage bucket.
#[cfg(feature = "openapi-spec")]
#[utoipa::path(
    get,
    path = "/storage/{agent_id}/{filename}",
    params(
        ("agent_id" = String, Path, description = "Agent identifier"),
        ("filename" = String, Path, description = "Stored filename")
    ),
    responses(
        (status = 200, description = "Stored file contents", content_type = "application/octet-stream"),
        (status = 404, description = "No such file")
    )
)]
#[must_use]
pub fn get_file() -> HttpResponse {
    HttpResponse::NotImplemented().finish()
}
