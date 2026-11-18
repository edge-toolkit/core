//! Covers the `/health` liveness-probe route: status code and response body shape.
#![cfg(test)]

use actix_web::http::header;
use actix_web::{App, test as actix_test, web};
use et_ws_server::health;
use et_ws_service::health::HealthResponse;
use pretty_assertions::assert_eq;

#[test_log::test(actix_rt::test)]
async fn health_returns_200_with_healthy_status() {
    let app = actix_test::init_service(App::new().route("/health", web::get().to(health))).await;

    let req = actix_test::TestRequest::get().uri("/health").to_request();
    let resp = actix_test::call_service(&app, req).await;

    assert!(resp.status().is_success());
    assert_eq!(resp.headers().get(header::CONTENT_TYPE).unwrap(), "application/json");

    let body: HealthResponse = actix_test::read_body_json(resp).await;
    assert_eq!(body.status, "healthy");
    assert_eq!(body.service, "ws-server");
}
