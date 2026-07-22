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
use tracing::{info, warn};

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
pub async fn put_file<S>(
    req: HttpRequest,
    mut payload: web::Payload,
    registry: web::Data<AgentRegistry<S>>,
    config: web::Data<StorageConfig>,
) -> Result<HttpResponse, StorageError>
where
    S: Clone + Send + 'static,
{
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

    let mut file = tokio::fs::File::create(&path).await?;
    let mut bytes_written: u64 = 0;
    while let Some(chunk) = payload.next().await {
        let chunk = chunk?;
        let copied = tokio::io::copy(&mut chunk.as_ref(), &mut file).await?;
        bytes_written = bytes_written.saturating_add(copied);
    }

    // This handler is the only path a file reaches storage through, so it is where every stored file can be
    // watched exactly once with an accurate byte count and zero extra I/O -- a separate filesystem watcher
    // would duplicate that work and risk observing a write mid-flight.
    if is_image_filename(&filename) {
        info!("Agent {} stored image {:?} ({} bytes)", agent_id, path, bytes_written);
        show_image_on_tty(&path);
    }

    Ok(HttpResponse::Ok().finish())
}

/// Render a thumbnail of the image at `path` directly to stdout, so the operator actually *sees* what was
/// stored rather than just its filename and byte count.
///
/// This bypasses `tracing` entirely and writes straight to the terminal: the escape sequences (or the
/// half-block art `viuer` falls back to) are tty-only presentation, not structured log data, and would
/// otherwise get shipped to the OTLP log exporter as log-record noise. Capped to 48 terminal columns so a
/// large capture doesn't flood the scrollback; decode/render failures (a corrupt upload, a non-terminal
/// stdout) only cost tty visibility, not the request, so they're reported to stderr rather than via `?`.
#[expect(
    clippy::single_call_fn,
    reason = "distinct step of put_file; kept separate for readability and testing"
)]
fn show_image_on_tty(path: &std::path::Path) {
    let config = viuer::Config {
        absolute_offset: false,
        width: Some(48),
        ..viuer::Config::default()
    };
    if let Err(error) = viuer::print_from_file(path, &config) {
        warn!("failed to render stored image {} to tty: {error}", path.display());
    }
}

/// Return whether `filename`'s extension marks it as an image, for the tty log line in [`put_file`].
///
/// Extension-only: the handler streams bytes straight to disk without buffering, so sniffing magic bytes
/// would need a peek-buffer or a post-write read-back, while the filename is already on hand for free.
#[must_use]
pub fn is_image_filename(filename: &std::path::Path) -> bool {
    const IMAGE_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];
    filename
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| IMAGE_EXTENSIONS.iter().any(|known| known.eq_ignore_ascii_case(ext)))
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
