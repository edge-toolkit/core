//! Integration tests for the `PUT /storage/{agent_id}/{filename}` route,
//! focused on the `StorageError` paths (`InvalidFilename` -> 400,
//! `AgentNotFound` -> 404, `Io` -> 500) plus the happy path.
//!
//! Each test brings up a fresh `App` with a tempdir-backed `StorageConfig`
//! and an `AgentRegistry` it controls explicitly, then hits the route via
//! `actix_web::test`. The same `configure` function the ws-server uses is
//! what wires the route into the test app.

#![cfg(test)]

use std::collections::BTreeMap;

use actix_web::dev::Payload as DevPayload;
use actix_web::error::ResponseError as _;
use actix_web::http::StatusCode;
use actix_web::{App, FromRequest as _, test, web};
use edge_toolkit::ws::AgentConnectionState;
use edge_toolkit::ws_server::{AgentRecord, AgentRegistry};
use et_storage_service::{StorageConfig, StorageError, configure, put_file};
use tempfile::TempDir;

/// Build a registry with a single connected agent.
fn registry_with_agent(agent_id: &str) -> AgentRegistry<()> {
    let mut agents = BTreeMap::new();
    let _previous: Option<AgentRecord<()>> = agents.insert(
        agent_id.to_string(),
        AgentRecord::new(AgentConnectionState::Connected, None, Some(())),
    );
    AgentRegistry::from_agents(agents)
}

fn storage_config(tmp: &TempDir) -> StorageConfig {
    StorageConfig::new(tmp.path().to_path_buf())
}

#[actix_rt::test]
async fn rejects_unknown_agent_with_404() {
    let tmp = tempfile::tempdir().unwrap();
    let config = storage_config(&tmp);
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(AgentRegistry::<()>::default()))
            .app_data(web::Data::new(config.clone()))
            .configure(|cfg| configure::<()>(cfg, &config)),
    )
    .await;

    let req = test::TestRequest::put()
        .uri("/storage/missing-agent/file.txt")
        .set_payload(b"hello".as_ref())
        .to_request();
    let resp = test::call_service(&app, req).await;

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// Invokes the handler directly, since a multi-component filename can't otherwise reach it.
///
/// `actix-router` doesn't decode `%2F` in path captures (it would collide with segment matching), so a
/// regular `PUT /storage/a/b%2Fc` request can't deliver a multi-component filename into the handler. Bypass
/// the router and invoke the handler directly with the parameters that the filter expects to reject
/// (`nested/path.txt` -> 2 components). This still exercises the same `StorageError::InvalidFilename` -> 400
/// path that the `ResponseError` impl wires up.
#[actix_rt::test]
async fn rejects_multi_component_filename_with_400() {
    let tmp = tempfile::tempdir().unwrap();
    let config = storage_config(&tmp);
    let registry = registry_with_agent("agent-1");

    let http_req = test::TestRequest::default()
        .param("agent_id", "agent-1")
        .param("filename", "nested/path.txt")
        .to_http_request();
    let mut payload = DevPayload::None;
    let payload = web::Payload::from_request(&http_req, &mut payload).await.unwrap();

    let result = put_file::<()>(http_req, payload, web::Data::new(registry), web::Data::new(config)).await;

    let err = result.unwrap_err();
    assert!(matches!(err, StorageError::InvalidFilename));
    assert_eq!(err.status_code(), StatusCode::BAD_REQUEST);
}

#[actix_rt::test]
async fn writes_file_for_registered_agent() {
    let tmp = tempfile::tempdir().unwrap();
    let config = storage_config(&tmp);
    let registry = registry_with_agent("agent-1");
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(registry))
            .app_data(web::Data::new(config.clone()))
            .configure(|cfg| configure::<()>(cfg, &config)),
    )
    .await;

    let body = b"hello from agent-1".as_ref();
    let req = test::TestRequest::put()
        .uri("/storage/agent-1/payload.txt")
        .set_payload(body)
        .to_request();
    let resp = test::call_service(&app, req).await;

    assert_eq!(resp.status(), StatusCode::OK);
    let written = fs_err::read(tmp.path().join("agent-1").join("payload.txt")).unwrap();
    assert_eq!(written, body);
}

/// Storing an image must succeed even though tty rendering cannot, since it is a best-effort side effect.
///
/// An image PUT under `cargo test` has no real terminal on stdout, so `tty_image::render`'s decode-and-render
/// step (triggered by the `.png` extension) is expected to fail internally. The route must still store the
/// file and return 200 regardless -- tty display is a best-effort side effect, never a reason to fail the upload.
#[actix_rt::test]
async fn stores_an_image_and_returns_200_even_though_tty_rendering_cannot_succeed_in_tests() {
    let tmp = tempfile::tempdir().unwrap();
    let config = storage_config(&tmp);
    let registry = registry_with_agent("agent-1");
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(registry))
            .app_data(web::Data::new(config.clone()))
            .configure(|cfg| configure::<()>(cfg, &config)),
    )
    .await;

    let body = b"\x89PNG\r\n\x1a\nnot a real png, just image-extension-shaped bytes".as_ref();
    let req = test::TestRequest::put()
        .uri("/storage/agent-1/capture.png")
        .set_payload(body)
        .to_request();
    let resp = test::call_service(&app, req).await;

    assert_eq!(resp.status(), StatusCode::OK);
    let written = fs_err::read(tmp.path().join("agent-1").join("capture.png")).unwrap();
    assert_eq!(written, body);
}

#[actix_rt::test]
async fn surfaces_io_failure_as_500() {
    // Point the storage root at a *file* (not a directory). The handler's
    // first I/O op is `create_dir_all`, which fails with `NotADirectory`
    // when one of the ancestors is a regular file. That propagates through
    // `StorageError::Io` and the derived `ResponseError` impl returns 500.
    let tmp = tempfile::tempdir().unwrap();
    let blocker = tmp.path().join("blocker");
    fs_err::write(&blocker, b"i am a file, not a directory").unwrap();
    let config = StorageConfig::new(blocker);
    let registry = registry_with_agent("agent-1");
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(registry))
            .app_data(web::Data::new(config.clone()))
            .configure(|cfg| configure::<()>(cfg, &config)),
    )
    .await;

    let req = test::TestRequest::put()
        .uri("/storage/agent-1/payload.txt")
        .set_payload(b"hello".as_ref())
        .to_request();
    let resp = test::call_service(&app, req).await;

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
