use std::net::{Ipv4Addr, SocketAddr};

use actix_web::{HttpResponse, web};
pub use et_ws_service::{AgentSession, WsAgentRegistry};

pub mod config;
pub mod routes;
pub mod tls;

pub use self::routes::health;
use crate::config::Config;

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

    et_ws_service::configure(cfg, &config.ws);
    et_storage_service::configure::<AgentSession>(cfg, &config.storage);
    // Relay for browser WASM runtimes (webR) that can only open a WebSocket: bridge /websockify to this
    // server's own plain-HTTP loopback port so their libcurl/httr2 can reach the storage API. Loopback-only
    // and server-fixed, so it is not an open proxy. Registered before the modules catch-all below.
    let relay_target = SocketAddr::from((
        Ipv4Addr::LOCALHOST,
        edge_toolkit::ports::Services::InsecureWebSocketServer.port(),
    ));
    et_websockify_service::configure(cfg, relay_target);
    // Must be last: registers a catch-all Files::new("/", ...) for the root module.
    et_modules_service::configure(cfg, &config.modules);
}
