pub mod config;

use std::path::{Path, PathBuf};

use actix_web::{HttpResponse, web};
pub use et_ws_service::{WebSocketActor, WsAgentRegistry};

use crate::config::Config;

pub fn browser_static_dir() -> PathBuf {
    Path::new(".").join("static")
}

pub async fn no_content() -> HttpResponse {
    HttpResponse::NoContent().finish()
}

pub async fn health() -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({
        "status": "healthy",
        "service": "ws-server"
    }))
}

pub fn configure_app(cfg: &mut web::ServiceConfig, agent_registry: web::Data<WsAgentRegistry>, config: &Config) {
    cfg.app_data(agent_registry)
        .app_data(web::Data::new(config.clone()))
        .app_data(web::Data::new(config.modules.clone()))
        .app_data(web::Data::new(config.storage.clone()))
        .route("/favicon.ico", web::get().to(no_content))
        .route("/health", web::get().to(health));

    et_ws_service::configure(cfg);
    et_storage_service::configure::<actix::Addr<WebSocketActor>>(cfg, &config.storage);
    // Must be last: registers a catch-all Files::new("/", ...) for the root module.
    et_modules_service::configure(cfg, &config.modules);
}
