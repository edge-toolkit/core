use std::path::PathBuf;
use std::sync::PoisonError;

use actix_files::Files;
use actix_web::web;
use actix_web_thiserror::ResponseError;
use serde::Deserialize;
use serde_default::DefaultFromSerde;
use thiserror::Error;

pub mod routes;

pub use self::routes::put_file;

/// Default storage directory.
#[must_use]
pub fn default_storage_folder() -> PathBuf {
    let project_root = edge_toolkit::config::get_project_root();
    project_root.join("services/ws-server/storage")
}

/// Storage config.
#[derive(Clone, Debug, DefaultFromSerde, Deserialize)]
#[non_exhaustive]
pub struct StorageConfig {
    #[serde(default = "default_storage_folder")]
    pub path: PathBuf,
}

impl StorageConfig {
    #[must_use]
    pub const fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

#[derive(Debug, Error, ResponseError)]
#[non_exhaustive]
pub enum StorageError {
    #[error("invalid filename")]
    #[response(status = 400, reason = "BAD_REQUEST")]
    InvalidFilename,

    #[error("agent not found")]
    #[response(status = 404, reason = "NOT_FOUND")]
    AgentNotFound,

    #[error("agent registry lock poisoned")]
    AgentRegistryPoisoned,

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Payload(#[from] actix_web::error::PayloadError),
}

// `PoisonError<T>` is generic over the guard type; a generic `From` impl
// lets `?` drop the `T` and surface the variant directly.
impl<T> From<PoisonError<T>> for StorageError {
    fn from(_source: PoisonError<T>) -> Self {
        Self::AgentRegistryPoisoned
    }
}

/// Register `PUT /storage/{agent_id}/{filename}` and `GET /storage/...` (static file serving).
pub fn configure<S: Clone + Send + 'static>(cfg: &mut web::ServiceConfig, config: &StorageConfig) {
    let storage_dir = config.path.clone();
    let _configured = cfg
        .route("/storage/{agent_id}/{filename}", web::put().to(put_file::<S>))
        .service(
            Files::new("/storage", storage_dir)
                .show_files_listing()
                .use_etag(true)
                .use_last_modified(true),
        );
}
