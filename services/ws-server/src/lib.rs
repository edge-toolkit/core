use actix_web::{HttpResponse, web};
pub use et_ws_service::{AgentSession, WsAgentRegistry};
use serde::{Deserialize, Serialize};

pub mod config;
mod openapi;

#[cfg(feature = "openapi-spec")]
pub use self::openapi::__path_health;
pub use self::openapi::health;
use crate::config::Config;

/// Server liveness probe response. Returned by `GET /health`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi-spec", derive(utoipa::ToSchema))]
#[non_exhaustive]
pub struct HealthResponse {
    pub status: String,
    pub service: String,
}

pub async fn no_content() -> HttpResponse {
    HttpResponse::NoContent().finish()
}

pub fn configure_app(cfg: &mut web::ServiceConfig, agent_registry: web::Data<WsAgentRegistry>, config: &Config) {
    let _configured = cfg
        .app_data(agent_registry)
        .app_data(web::Data::new(config.clone()))
        .app_data(web::Data::new(config.modules.clone()))
        .app_data(web::Data::new(config.storage.clone()))
        .route("/favicon.ico", web::get().to(no_content))
        .route("/health", web::get().to(health));

    et_ws_service::configure(cfg);
    et_storage_service::configure::<AgentSession>(cfg, &config.storage);
    // Must be last: registers a catch-all Files::new("/", ...) for the root module.
    et_modules_service::configure(cfg, &config.modules);
}
