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
use actix_web::http::{Method, StatusCode, header};
use actix_web::{App, FromRequest as _, test, web};
use edge_toolkit::ws::AgentConnectionState;
use edge_toolkit::ws_server::{AgentRecord, AgentRegistry};
use et_storage_service::{StorageConfig, StorageError, build_store, configure, put_file};
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
    StorageConfig::local(tmp.path())
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

    let store = web::Data::<dyn object_store::ObjectStore>::from(build_store(&config).unwrap());
    let result = put_file::<()>(http_req, payload, web::Data::new(registry), store).await;

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

/// PUT returns an `ETag`, and GET and HEAD report the same entity tag; HEAD also reports the object size.
///
/// Exercises the S3-compatible surface the storage service exposes: an S3 client stats an object with `HEAD`
/// (entity tag + `Content-Length`, no body) and reads its `ETag` off `GET`. The GET and HEAD tags are compared
/// to each other because both come from the stored object's metadata, so they match regardless of how a given
/// backend derives the tag on write.
#[actix_rt::test]
async fn get_and_head_expose_etag_and_head_reports_size() {
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

    let body = b"etag round-trip payload".as_ref();
    let put = test::TestRequest::put()
        .uri("/storage/agent-1/payload.txt")
        .set_payload(body)
        .to_request();
    let put_resp = test::call_service(&app, put).await;
    assert_eq!(put_resp.status(), StatusCode::OK);
    assert!(
        put_resp.headers().contains_key(header::ETAG),
        "PUT should return an ETag"
    );

    let get = test::TestRequest::get()
        .uri("/storage/agent-1/payload.txt")
        .to_request();
    let get_resp = test::call_service(&app, get).await;
    assert_eq!(get_resp.status(), StatusCode::OK);
    let get_etag = get_resp.headers().get(header::ETAG).cloned();
    assert!(get_etag.is_some(), "GET should return an ETag");

    let head = test::TestRequest::default()
        .method(Method::HEAD)
        .uri("/storage/agent-1/payload.txt")
        .to_request();
    let head_resp = test::call_service(&app, head).await;
    assert_eq!(head_resp.status(), StatusCode::OK);
    assert_eq!(
        head_resp.headers().get(header::ETAG).cloned(),
        get_etag,
        "HEAD ETag should match GET"
    );
    assert_eq!(
        head_resp
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok()),
        Some(body.len().to_string().as_str()),
        "HEAD should report the object size as Content-Length"
    );
}

/// HEAD on an object that was never stored is a 404, like GET.
#[actix_rt::test]
async fn head_missing_object_returns_404() {
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

    let head = test::TestRequest::default()
        .method(Method::HEAD)
        .uri("/storage/agent-1/absent.txt")
        .to_request();
    let resp = test::call_service(&app, head).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[actix_rt::test]
async fn surfaces_io_failure_as_500() {
    // Block the *agent's* directory with a regular file, so the store's write fails on an ancestor that is not
    // a directory. That surfaces as `StorageError::Store` and the derived `ResponseError` impl returns 500.
    //
    // The storage root itself stays a valid directory on purpose: an unusable root is a misconfigured
    // deployment, which `configure` now rejects at startup rather than turning into a per-request 500, so
    // pointing the root at a file (what this test used to do) would panic before any request was served.
    // Blocking one level down keeps the test on the request path it is actually about.
    let tmp = tempfile::tempdir().unwrap();
    fs_err::write(tmp.path().join("agent-1"), b"i am a file, not a directory").unwrap();
    let config = StorageConfig::local(tmp.path());
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
