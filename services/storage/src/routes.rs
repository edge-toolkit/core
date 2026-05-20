//! Real `PUT /storage/{agent_id}/{filename}` handler.
//!
//! The `OpenAPI` spec for this route lives separately in `openapi.rs` as a
//! placeholder annotated with `#[utoipa::path]`; that split keeps the
//! `clippy::exhaustive_structs` exception (forced by utoipa's macro-internal
//! `__path_*` struct) scoped to a single file.

use std::path::PathBuf;

use actix_web::{HttpRequest, HttpResponse, web};
use edge_toolkit::ws_server::AgentRegistry;
use futures_util::StreamExt as _;
use tracing::info;

use crate::{StorageConfig, StorageError};

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
    std::fs::create_dir_all(&agent_dir)?;

    let path = agent_dir.join(&filename);
    info!("Agent {} storing file: {:?}", agent_id, path);

    let mut file = tokio::fs::File::create(path).await?;
    while let Some(chunk) = payload.next().await {
        let chunk = chunk?;
        let _copied: u64 = tokio::io::copy(&mut chunk.as_ref(), &mut file).await?;
    }

    Ok(HttpResponse::Ok().finish())
}
