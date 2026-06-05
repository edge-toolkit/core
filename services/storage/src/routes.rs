//! Storage HTTP routes carrying `#[utoipa::path]` annotations.
//!
//! `put_file` is the live PUT handler; `get_file` is a fake stub whose
//! only role is to host the `#[utoipa::path]` annotation for the GET
//! route (actually served by an `actix_files::Files` mount registered
//! in [`crate::configure`]). `BinaryBlob` is the phantom request-body
//! schema both routes reference.
//!
//! The file-level `#![expect(clippy::exhaustive_structs)]` (active
//! under `openapi-spec`) is scoped here because utoipa's derives emit
//! `pub struct`s without `#[non_exhaustive]` -- and the lint locations
//! aren't reachable through a function- or item-level `#[expect]`.

#![cfg_attr(
    feature = "openapi-spec",
    expect(
        clippy::exhaustive_structs,
        reason = "utoipa emits sibling pub structs without #[non_exhaustive]; only file scope can silence the lint"
    )
)]

use std::path::PathBuf;

use actix_web::{HttpRequest, HttpResponse, web};
use edge_toolkit::ws_server::AgentRegistry;
use futures_util::StreamExt as _;
use tracing::info;

use crate::{StorageConfig, StorageError};

/// Phantom type used to label binary request/response bodies as `string`/`binary`.
///
/// Never constructed at runtime; only exists under the `openapi-spec` feature
/// so the `utoipa::ToSchema` derive has something to attach to.
#[cfg(feature = "openapi-spec")]
#[derive(utoipa::ToSchema)]
#[schema(value_type = String, format = Binary)]
pub struct BinaryBlob(#[expect(dead_code)] Vec<u8>);

/// Upload a file to an agent's storage bucket.
///
/// Only the agent that owns the bucket may write to it (the agent must
/// currently be connected); the path component must be a single
/// filename, not a nested path.
#[cfg_attr(
    feature = "openapi-spec",
    utoipa::path(
        put,
        path = "/storage/{agent_id}/{filename}",
        tag = "storage",
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
    )
)]
#[expect(
    clippy::future_not_send,
    reason = "actix-web Payload is !Send by design; handler runs on actix's single-threaded runtime"
)]
pub async fn put_file<S: Clone + Send + 'static>(
    req: HttpRequest,
    mut payload: web::Payload,
    registry: web::Data<AgentRegistry<S>>,
    config: web::Data<StorageConfig>,
) -> Result<HttpResponse, StorageError> {
    let agent_id = req.match_info().query("agent_id").to_string();
    let filename = req
        .match_info()
        .query("filename")
        .parse::<PathBuf>()
        .ok()
        .filter(|filename| filename.components().count() == 1)
        .ok_or(StorageError::InvalidFilename)?;

    if !registry.agents.lock()?.contains_key(&agent_id) {
        return Err(StorageError::AgentNotFound);
    }

    let storage_dir = &config.path;
    let agent_dir = storage_dir.join(&agent_id);
    fs_err::create_dir_all(&agent_dir)?;

    let path = agent_dir.join(&filename);
    info!("Agent {} storing file: {:?}", agent_id, path);

    let mut file = tokio::fs::File::create(path).await?;
    while let Some(chunk) = payload.next().await {
        let chunk = chunk?;
        let _copied: u64 = tokio::io::copy(&mut chunk.as_ref(), &mut file).await?;
    }

    Ok(HttpResponse::Ok().finish())
}

/// Download a file previously written to the named agent's storage bucket.
#[cfg(feature = "openapi-spec")]
#[utoipa::path(
    get,
    path = "/storage/{agent_id}/{filename}",
    tag = "storage",
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
    // Fake handler -- the GET route is actually served by the
    // `actix_files::Files` mount registered in `crate::configure`; this
    // stub exists only to host the `#[utoipa::path]` annotation so
    // `et-int-gen` can include the GET route in `generated/specs/rest.yaml`.
    HttpResponse::NotImplemented().finish()
}
