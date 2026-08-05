//! Covers `no_content` and `configure_app`'s route wiring: favicon, health, and delegation to the
//! sub-services `configure_app` chains together.
#![cfg(test)]

use actix_web::http::StatusCode;
use actix_web::{App, test, web};
use et_storage_service::StorageConfig;
use et_ws_server::config::Config;
use et_ws_server::{WsAgentRegistry, configure_app, no_content};
use tempfile::tempdir;

#[actix_rt::test]
async fn no_content_returns_204() {
    let resp = no_content().await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[actix_rt::test]
async fn configure_app_wires_favicon_health_and_modules() {
    let storage_dir = tempdir().unwrap();
    let mut config = Config::default();
    config.storage = StorageConfig::local(storage_dir.path());

    let registry = web::Data::new(WsAgentRegistry::default());
    let app = test::init_service(App::new().configure(|cfg| configure_app(cfg, registry, &config))).await;

    let favicon_req = test::TestRequest::get().uri("/favicon.ico").to_request();
    let favicon_resp = test::call_service(&app, favicon_req).await;
    assert_eq!(favicon_resp.status(), StatusCode::NO_CONTENT);

    let health_req = test::TestRequest::get().uri("/health").to_request();
    let health_resp = test::call_service(&app, health_req).await;
    assert!(health_resp.status().is_success());

    // `/modules/` only resolves if `et_modules_service::configure` found and served the root module,
    // which only runs if every configure call before it in `configure_app` (ws, storage, websockify)
    // succeeded -- so a 200 here is proof the whole wiring chain ran.
    let modules_req = test::TestRequest::get().uri("/modules/").to_request();
    let modules_resp = test::call_service(&app, modules_req).await;
    assert_eq!(modules_resp.status(), StatusCode::OK);
}
