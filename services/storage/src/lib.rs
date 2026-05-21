use std::path::PathBuf;

use actix_files::Files;
use actix_web::{HttpRequest, HttpResponse, web};
use actix_web_thiserror::ResponseError;
use edge_toolkit::ws_server::AgentRegistry;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_default::DefaultFromSerde;
use thiserror::Error;
use tracing::info;

/// Default storage directory.
#[must_use]
pub fn default_storage_folder() -> PathBuf {
    let project_root = edge_toolkit::config::get_project_root();
    project_root.join("services/ws-server/storage")
}

/// Storage config.
#[derive(Clone, Debug, DefaultFromSerde, Deserialize)]
pub struct StorageConfig {
    #[serde(default = "default_storage_folder")]
    pub path: PathBuf,
}

#[derive(Debug, Error, ResponseError)]
pub enum StorageError {
    #[error("invalid filename")]
    #[response(status = 400, reason = "BAD_REQUEST")]
    InvalidFilename,

    #[error("agent not found")]
    #[response(status = 404, reason = "NOT_FOUND")]
    AgentNotFound,

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Payload(#[from] actix_web::error::PayloadError),
}

pub async fn agent_put_file<S: Clone + Send + 'static>(
    req: HttpRequest,
    mut payload: web::Payload,
    registry: web::Data<AgentRegistry<S>>,
    config: web::Data<StorageConfig>,
) -> Result<HttpResponse, StorageError> {
    let agent_id: String = req.match_info().query("agent_id").parse().unwrap();
    let filename = req
        .match_info()
        .query("filename")
        .parse::<PathBuf>()
        .ok()
        .filter(|filename| filename.components().count() == 1)
        .ok_or(StorageError::InvalidFilename)?;

    {
        let agents = registry.agents.lock().expect("lock poisoned");
        if !agents.contains_key(&agent_id) {
            return Err(StorageError::AgentNotFound);
        }
    }

    let storage_dir = &config.path;
    let agent_dir = storage_dir.join(&agent_id);
    std::fs::create_dir_all(&agent_dir)?;

    let path = agent_dir.join(&filename);
    info!("Agent {} storing file: {:?}", agent_id, path);

    let mut file = tokio::fs::File::create(path).await?;
    while let Some(chunk) = payload.next().await {
        let chunk = chunk?;
        tokio::io::copy(&mut &chunk[..], &mut file).await?;
    }

    Ok(HttpResponse::Ok().finish())
}

/// Register `PUT /storage/{agent_id}/{filename}` and `GET /storage/...` (static file serving).
pub fn configure<S: Clone + Send + 'static>(cfg: &mut web::ServiceConfig, config: &StorageConfig) {
    let storage_dir = config.path.clone();
    cfg.route("/storage/{agent_id}/{filename}", web::put().to(agent_put_file::<S>))
        .service(
            Files::new("/storage", storage_dir)
                .show_files_listing()
                .use_etag(true)
                .use_last_modified(true),
        );
}
