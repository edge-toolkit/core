//! Agent file storage, backed by any `object_store` backend.
//!
//! The wire protocol is unchanged (`PUT`/`GET /storage/{agent_id}/{filename}`); only the storage layer beneath
//! it is pluggable. [`StorageConfig::url`] selects the backend and defaults to a `file://` URL under
//! [`default_storage_folder`], so nothing needs configuring for research use; an `object_store` URL such as
//! `s3://bucket` points at a remote instead. Objects are addressed as `<agent_id>/<filename>` under whichever
//! store is in use, so the local-disk layout is the same as before.

use std::path::PathBuf;
use std::sync::{Arc, PoisonError};

use actix_web::web;
use actix_web_thiserror::ResponseError;
use object_store::ObjectStore;
use object_store::local::LocalFileSystem;
use serde::Deserialize;
use serde_default::DefaultFromSerde;
use thiserror::Error;

pub mod routes;
mod tty_image;

pub use self::routes::{get_file, put_file};

/// Default storage directory.
#[must_use]
pub fn default_storage_folder() -> PathBuf {
    let project_root = edge_toolkit::config::get_project_root();
    project_root.join("services/ws-server/storage")
}

/// `file://` URL for [`default_storage_folder`], the backend used when nothing is configured.
///
/// This is the default baked into [`StorageConfig::url`], so the local-disk backend is expressed the same way
/// as any other -- as a URL -- rather than being inferred later from an absent field.
#[must_use]
pub fn default_storage_url() -> String {
    file_url(&default_storage_folder())
}

/// Render `path` as a `file://` URL.
///
/// `Url::from_directory_path` only fails for a non-absolute path, and every caller here passes an absolute one
/// (the project root, or a tempdir). The fallback keeps this infallible rather than forcing a `Result` through
/// serde's default hook, and is not reachable for an absolute input on any platform.
#[must_use]
pub fn file_url(path: &std::path::Path) -> String {
    url::Url::from_directory_path(path).map_or_else(|()| format!("file://{}", path.display()), String::from)
}

/// Storage config.
#[derive(Clone, Debug, DefaultFromSerde, Deserialize)]
#[non_exhaustive]
pub struct StorageConfig {
    /// `object_store` backend URL, defaulting to local disk under [`default_storage_folder`].
    ///
    /// One field rather than a path plus an optional override: the backend has exactly one source of truth, and
    /// the default is declared here rather than reconstructed inside [`build_store`]. Anything `object_store`
    /// recognises for a compiled-in backend works -- a `file:` URL naming an absolute directory, `s3://bucket`
    /// (including S3-compatible servers), `memory://`. Credentials, endpoint and addressing style come from each
    /// backend's own standard environment variables rather than config keys of our own, so an operator configures
    /// a store exactly as they would for any other client of it.
    #[serde(default = "default_storage_url")]
    pub url: String,
}

impl StorageConfig {
    /// Build a config for an explicit backend URL.
    #[must_use]
    pub const fn new(url: String) -> Self {
        Self { url }
    }

    /// Build a config for a local directory, for callers holding a path rather than a URL.
    #[must_use]
    pub fn local(path: &std::path::Path) -> Self {
        Self { url: file_url(path) }
    }
}

/// Shared handle to the configured backend, held in actix app data and by the routes.
pub type SharedStore = Arc<dyn ObjectStore>;

#[derive(Debug, Error, ResponseError)]
#[non_exhaustive]
pub enum StorageError {
    #[error("invalid filename")]
    #[response(status = 400, reason = "BAD_REQUEST")]
    InvalidFilename,

    #[error("agent not found")]
    #[response(status = 404, reason = "NOT_FOUND")]
    AgentNotFound,

    #[error("no such object")]
    #[response(status = 404, reason = "NOT_FOUND")]
    ObjectNotFound,

    #[error("agent registry lock poisoned")]
    AgentRegistryPoisoned,

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Payload(#[from] actix_web::error::PayloadError),

    #[error(transparent)]
    Store(#[from] object_store::Error),

    #[error(transparent)]
    StoreUrl(#[from] url::ParseError),
}

// `PoisonError<T>` is generic over the guard type; a generic `From` impl
// lets `?` drop the `T` and surface the variant directly.
impl<T> From<PoisonError<T>> for StorageError {
    fn from(_source: PoisonError<T>) -> Self {
        Self::AgentRegistryPoisoned
    }
}

/// Build the backend described by `config`.
///
/// Remote backends are configured from the process environment, so credentials can be supplied as separate
/// environment variables (a mounted Kubernetes secret, say) instead of being embedded in the URL. `object_store`
/// errors clearly if the URL names a backend whose feature is not compiled in -- currently only `aws`, plus the
/// always-present `file` and `memory`.
pub fn build_store(config: &StorageConfig) -> Result<SharedStore, StorageError> {
    let parsed = url::Url::parse(&config.url)?;

    // `file` is the one scheme handled here rather than by object_store's own dispatch.
    // `parse_url_opts` maps it to `LocalFileSystem::new()` -- no prefix -- which would resolve every
    // `<agent_id>/<filename>` key against the filesystem root instead of the configured directory. Prefixing the
    // store is what makes keys relative, matching how every remote backend already behaves, and the directory
    // has to exist first for `new_with_prefix` to accept it (which also preserves first-run behaviour).
    if parsed.scheme() == "file" {
        let root = parsed.to_file_path().unwrap_or_else(|()| PathBuf::from(parsed.path()));
        fs_err::create_dir_all(&root)?;
        return Ok(Arc::new(LocalFileSystem::new_with_prefix(&root)?));
    }

    // Every other scheme gets the process environment as options, which is object_store's documented pattern
    // (see `parse_url_opts`) and the reason each backend can be configured -- credentials included -- from
    // separate environment variables rather than from secrets embedded in the URL. `builder_opts!` lowercases
    // each key and silently skips ones the backend does not recognise, so the whole environment can be handed
    // over: `AWS_SECRET_ACCESS_KEY`, `GOOGLE_SERVICE_ACCOUNT_KEY`, `AZURE_STORAGE_ACCOUNT_KEY` and friends each
    // land on their own backend, which is what makes a Kubernetes secret mounted as an env var work here.
    // Doing this per-scheme with each builder's own `from_env` would have given S3 that treatment and quietly
    // denied it to the others.
    let (store, _prefix) = object_store::parse_url_opts(&parsed, std::env::vars())?;
    Ok(Arc::from(store))
}

/// Register `PUT /storage/{agent_id}/{filename}` and `GET /storage/{agent_id}/{filename}`.
///
/// # Panics
///
/// Panics if `config` describes a backend that cannot be constructed (an unparsable URL, a scheme whose
/// backend is not compiled in, or an unusable local path). That is a misconfigured deployment rather than a
/// runtime condition, so it fails at startup instead of turning every later request into a 500.
#[expect(
    clippy::panic,
    reason = "an unbuildable backend is a misconfigured deployment; fail at startup, not on every request"
)]
pub fn configure<S>(cfg: &mut web::ServiceConfig, config: &StorageConfig)
where
    S: Clone + Send + 'static,
{
    let store = match build_store(config) {
        Ok(store) => store,
        Err(error) => panic!("storage backend is misconfigured: {error}"),
    };
    let _configured = cfg
        .app_data(web::Data::<dyn ObjectStore>::from(store))
        .route("/storage/{agent_id}/{filename}", web::put().to(put_file::<S>))
        .route("/storage/{agent_id}/{filename}", web::get().to(get_file));
}
