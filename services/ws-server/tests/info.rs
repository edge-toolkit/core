//! Covers the `/info` build-information route: status code and the build it reports.
#![cfg(test)]

use actix_web::{App, test, web};
use et_ws_server::info;
use et_ws_server::routes::BuildInfo;
use pretty_assertions::assert_eq;
use pretty_assertions::assert_ne;

#[actix_rt::test]
async fn info_reports_this_build() {
    let app = test::init_service(App::new().route("/info", web::get().to(info))).await;

    let req = test::TestRequest::get().uri("/info").to_request();
    let resp = test::call_service(&app, req).await;

    assert!(resp.status().is_success());
    let body: BuildInfo = test::read_body_json(resp).await;
    assert_eq!(body.name, "et-ws-server");
    assert_eq!(body.version, env!("CARGO_PKG_VERSION"));
    assert!(body.rustc.starts_with("rustc "), "got: {}", body.rustc);
    assert_ne!(body.target, "");
    assert!(
        body.features.iter().any(|feature| feature == "default"),
        "got: {:?}",
        body.features
    );
}
