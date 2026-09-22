//! Covers what `configure` does about `config.root`: a name none of the scanned `config.paths` provides is a
//! misconfiguration that must surface at startup rather than as a silent 404, while naming nothing at all is
//! the ordinary case for a deployment with no front page and must serve the module routes regardless.
#![cfg(test)]

use actix_web::http::StatusCode;
use actix_web::{App, test, web};
use et_modules_service::{ModulesConfig, configure};

#[actix_rt::test]
#[should_panic(expected = "Root module 'nonexistent-root' not found")]
async fn configure_panics_when_root_module_is_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let config = ModulesConfig::new(vec![tmp.path().to_path_buf()], "nonexistent-root".to_string());

    let _app = test::init_service(App::new().configure(|cfg| configure(cfg, &config))).await;
}

#[actix_rt::test]
async fn an_unset_root_serves_the_module_routes_and_nothing_at_the_root() {
    let tmp = tempfile::tempdir().unwrap();
    let config = ModulesConfig::new(vec![tmp.path().to_path_buf()], String::default());

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(config.clone()))
            .configure(|cfg| configure(cfg, &config)),
    )
    .await;

    let listing = test::call_service(&app, test::TestRequest::get().uri("/modules/").to_request()).await;
    assert_eq!(
        listing.status(),
        StatusCode::OK,
        "the module listing is the whole surface"
    );

    // Nothing is mounted at `/`, so the request falls through to actix's own default rather than an index
    // page -- which is the point: no front page was asked for, and none was invented.
    let root = test::call_service(&app, test::TestRequest::get().uri("/").to_request()).await;
    assert_eq!(root.status(), StatusCode::NOT_FOUND, "no module was named for /");
}
