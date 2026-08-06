//! Storage HTTP routes carrying `#[utoipa::path]` annotations.
//!
//! Both handlers are live and go through the configured `object_store`
//! backend, so the same code serves local disk and any remote store.
//! `get_file` used to be a stub hosting only the `#[utoipa::path]`
//! annotation, with an `actix_files::Files` mount doing the real work --
//! that mount could only ever read local disk, so it is gone.
//! `BinaryBlob` is the phantom request-body schema both routes
//! reference.
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

use actix_web::http::header;
use actix_web::{HttpRequest, HttpResponse, HttpResponseBuilder, web};
use edge_toolkit::ws_server::AgentRegistry;
use futures_util::StreamExt as _;
use object_store::{ObjectStore, ObjectStoreExt as _, PutPayload};
use tracing::{info, warn};

use crate::StorageError;

/// Object key for `filename` inside `agent_id`'s bucket.
///
/// Every backend addresses objects the same way, so this is the one place the `<agent_id>/<filename>` layout is
/// spelled out -- it is also what keeps a local-disk store laid out exactly as the previous fs implementation.
fn object_path(agent_id: &str, filename: &std::path::Path) -> object_store::path::Path {
    object_store::path::Path::from(format!("{agent_id}/{}", filename.display()))
}

/// Extract and validate the `{agent_id}`/`{filename}` pair from the request path.
///
/// The filename must be a single path component: a nested or absolute path would let a caller address objects
/// outside its own bucket.
fn agent_and_filename(req: &HttpRequest) -> Result<(String, PathBuf), StorageError> {
    let agent_id = req.match_info().query("agent_id").to_string();
    let filename = req
        .match_info()
        .query("filename")
        .parse::<PathBuf>()
        .ok()
        .filter(|filename| filename.components().count() == 1)
        .ok_or(StorageError::InvalidFilename)?;

    Ok((agent_id, filename))
}

/// Attach the object's `ETag` header to a response when the backend reported one.
///
/// `object_store` surfaces an entity tag on writes (`PutResult`) and reads (`ObjectMeta`) for every backend that
/// has one -- the S3 MD5/opaque tag, or a size+mtime tag for local disk -- and `None` only when a backend cannot
/// produce one. Emitting it lets S3 clients (and HTTP conditional requests) observe the same tag the store holds.
fn insert_etag(response: &mut HttpResponseBuilder, e_tag: Option<&str>) {
    if let Some(etag) = e_tag {
        let _inserted = response.insert_header((header::ETAG, etag));
    }
}

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
            (status = 200, description = "File stored", headers(
                ("ETag" = String, description = "Entity tag of the stored object")
            )),
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
    store: web::Data<dyn ObjectStore>,
) -> Result<HttpResponse, StorageError>
where
    S: Clone + Send + 'static,
{
    let (agent_id, filename) = agent_and_filename(&req)?;

    if !registry.agents.lock()?.contains_key(&agent_id) {
        return Err(StorageError::AgentNotFound);
    }

    // The body is buffered rather than streamed to the store.
    // `ObjectStore::put` takes a whole payload, and the alternative (`put_multipart`) buys streaming at the
    // cost of chunk bookkeeping that these payloads -- module output and captured frames -- do not need. The
    // buffer also gives the tty preview below the bytes for free, where the previous fs implementation could
    // rely on re-reading the file it had just written.
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = payload.next().await {
        body.extend_from_slice(&chunk?);
    }
    let bytes_written = body.len();

    let path = object_path(&agent_id, &filename);
    info!("Agent {} storing file: {}", agent_id, path);
    let put_result = store.put(&path, PutPayload::from(body.clone())).await?;

    // This handler is the only path a file reaches storage through, so it is where every stored file can be
    // watched exactly once with an accurate byte count and zero extra I/O -- a separate filesystem watcher
    // would duplicate that work, risk observing a write mid-flight, and could not see a remote backend at all.
    if is_image_filename(&filename) {
        info!("Agent {} stored image {} ({} bytes)", agent_id, path, bytes_written);
        show_image_on_tty(&body);
    }

    let mut response = HttpResponse::Ok();
    insert_etag(&mut response, put_result.e_tag.as_deref());
    Ok(response.finish())
}

/// Render a thumbnail of the just-stored image bytes directly to stdout.
///
/// This lets the operator actually *see* what was stored, rather than just its filename and byte count. It
/// bypasses `tracing` entirely and writes straight to the terminal: the escape sequences (ANSI
/// truecolor half-block art -- see the `tty_image` module) are tty-only presentation, not structured log
/// data, and would otherwise get shipped to the OTLP log exporter as log-record noise. Decode/render
/// failures (a corrupt upload, a non-terminal stdout) only cost tty visibility, not the request, so they're
/// reported via `warn!` rather than via `?`.
///
/// Takes bytes rather than a path because the object may never exist on the local filesystem -- with a remote
/// backend there is nothing to re-read.
#[expect(
    clippy::single_call_fn,
    reason = "distinct step of put_file; kept separate for readability and testing"
)]
fn show_image_on_tty(bytes: &[u8]) {
    if let Err(error) = crate::tty_image::render_bytes(bytes) {
        warn!("failed to render stored image to tty: {error}");
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
#[cfg_attr(
    feature = "openapi-spec",
    utoipa::path(
        get,
        path = "/storage/{agent_id}/{filename}",
        tag = "storage",
        params(
            ("agent_id" = String, Path, description = "Agent identifier"),
            ("filename" = String, Path, description = "Stored filename")
        ),
        responses(
            (status = 200, description = "Stored file contents", content_type = "application/octet-stream", headers(
                ("ETag" = String, description = "Entity tag of the stored object")
            )),
            (status = 404, description = "No such file")
        )
    )
)]
#[expect(
    clippy::future_not_send,
    reason = "actix-web HttpRequest is !Send by design; handler runs on actix's single-threaded runtime"
)]
pub async fn get_file(req: HttpRequest, store: web::Data<dyn ObjectStore>) -> Result<HttpResponse, StorageError> {
    let (agent_id, filename) = agent_and_filename(&req)?;
    let path = object_path(&agent_id, &filename);

    // A missing object is a 404, not a 500, and it is the one store error worth distinguishing. Matched rather
    // than mapped because `.map_err` is banned outside this workspace's designated error-wrapper modules.
    let object = match store.get(&path).await {
        Ok(object) => object,
        Err(object_store::Error::NotFound { .. }) => return Err(StorageError::ObjectNotFound),
        Err(error) => return Err(error.into()),
    };
    // Read the entity tag off the metadata before `bytes()` consumes the `GetResult`.
    let e_tag = object.meta.e_tag.clone();
    let body = object.bytes().await?;

    let mut response = HttpResponse::Ok();
    let _typed = response.content_type("application/octet-stream");
    insert_etag(&mut response, e_tag.as_deref());
    Ok(response.body(body))
}

/// Return a stored object's metadata without its body (S3 `HeadObject`).
///
/// Same addressing and 404 handling as [`get_file`], but the response carries headers only: the object's `ETag`
/// and its size as `Content-Length`. S3 clients issue `HEAD` to stat an object (existence, size, entity tag)
/// before downloading, so it reports the same `ETag` a `GET` would.
#[cfg_attr(
    feature = "openapi-spec",
    utoipa::path(
        head,
        path = "/storage/{agent_id}/{filename}",
        tag = "storage",
        params(
            ("agent_id" = String, Path, description = "Agent identifier"),
            ("filename" = String, Path, description = "Stored filename")
        ),
        responses(
            (status = 200, description = "Object metadata (no body)", headers(
                ("ETag" = String, description = "Entity tag of the stored object"),
                ("Content-Length" = i64, description = "Size of the stored object in bytes")
            )),
            (status = 404, description = "No such file")
        )
    )
)]
#[expect(
    clippy::future_not_send,
    reason = "actix-web HttpRequest is !Send by design; handler runs on actix's single-threaded runtime"
)]
pub async fn head_file(req: HttpRequest, store: web::Data<dyn ObjectStore>) -> Result<HttpResponse, StorageError> {
    let (agent_id, filename) = agent_and_filename(&req)?;
    let path = object_path(&agent_id, &filename);

    let meta = match store.head(&path).await {
        Ok(meta) => meta,
        Err(object_store::Error::NotFound { .. }) => return Err(StorageError::ObjectNotFound),
        Err(error) => return Err(error.into()),
    };

    // `no_chunking` sets Content-Length to the object's size so a `HEAD` reports it exactly as the matching
    // `GET` would, without sending a body.
    let mut response = HttpResponse::Ok();
    let _typed = response.content_type("application/octet-stream");
    let _sized = response.no_chunking(meta.size);
    insert_etag(&mut response, meta.e_tag.as_deref());
    Ok(response.finish())
}
