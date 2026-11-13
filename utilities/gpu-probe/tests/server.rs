//! Covers the server's routes, served in process: the form, the registry, static files, and each answer `/register`
//! gives -- and the names a generated certificate covers.
#![cfg(test)]

use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;

use actix_web::dev::{Service, ServiceResponse};
use actix_web::http::StatusCode;
use actix_web::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use actix_web::test as served;
use actix_web::{App, web};
use et_gpu_probe_server::registry::Registry;
use et_gpu_probe_server::{advertised_names, app, nip_io};
use serde_json::Value;

/// A registry holding one known signature.
const REGISTRY: &str = r#"signatures:
  - interface: webgpu-split
    label: "Known GPU"
    seen:
      - browser: chrome
        os: macos
        cpu_arch: aarch64
        gpu: apple
        backend: metal
        window: normal
    results:
      fma: "33800000"
"#;

/// A submission of a new signature, as a page posts it.
const NEW: &str = r#"{"interface": "webgpu-clear", "label": "New", "results": {"a": "1"},
  "seen": {"browser": "firefox", "os": "linux", "cpu_arch": "x86_64", "gpu": "intel", "backend": "vulkan",
  "window": "normal"}}"#;

/// The server's routes, adding to a registry at `path` and serving the files under `root`.
#[expect(
    clippy::future_not_send,
    reason = "actix serves in process on the test's own thread, and its service is not Send"
)]
async fn service(
    path: PathBuf,
    root: Option<PathBuf>,
) -> impl Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error> {
    let registry = web::Data::new(Registry::new(path));
    served::init_service(App::new().configure(|config| app::configure(config, registry, root))).await
}

/// A directory holding a registry file of [`REGISTRY`] and a probe page, removed when the guard drops.
fn workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs_err::write(dir.path().join("signatures.yaml"), REGISTRY).unwrap();
    fs_err::write(dir.path().join("index.html"), "<p>probe page</p>").unwrap();
    dir
}

/// What `/register` answers `body`, sent as JSON, with: the status, and the JSON reply's `ok` and `message`.
#[expect(
    clippy::future_not_send,
    reason = "actix serves in process on the test's own thread, and its service is not Send"
)]
async fn register(path: PathBuf, body: &str) -> (StatusCode, bool, String) {
    register_as(path, body, "application/json").await
}

/// What `/register` answers `body`, sent as `content_type`, with: the status, and the reply's `ok` and `message`.
#[expect(
    clippy::future_not_send,
    reason = "actix serves in process on the test's own thread, and its service is not Send"
)]
async fn register_as(path: PathBuf, body: &str, content_type: &str) -> (StatusCode, bool, String) {
    let app = service(path, None).await;
    let request = served::TestRequest::post()
        .uri("/register")
        .insert_header(("content-type", content_type))
        .set_payload(body.to_owned())
        .to_request();
    let response = served::call_service(&app, request).await;
    let status = response.status();
    let reply: Value = served::read_body_json(response).await;
    (
        status,
        reply["ok"].as_bool().unwrap(),
        reply["message"].as_str().unwrap().to_owned(),
    )
}

#[actix_rt::test]
async fn the_form_and_what_the_pages_share_are_served_and_never_cached() {
    let dir = workspace();
    let app = service(dir.path().join("signatures.yaml"), None).await;
    let built_in = [
        ("/submit", "text/html", "import { register } from \"/register.js\""),
        ("/probe.css", "text/css", ".bad {"),
        ("/register.js", "text/javascript", "fetch(\"/register\""),
    ];
    for (path, kind, holds) in built_in {
        let response = served::call_service(&app, served::TestRequest::get().uri(path).to_request()).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers().get(CACHE_CONTROL).unwrap(), "no-store");
        assert!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with(kind)
        );
        let body = served::read_body(response).await;
        assert!(String::from_utf8_lossy(&body).contains(holds), "{path} holds {holds}");
    }
}

#[actix_rt::test]
async fn the_registry_is_served_as_it_stands() {
    let dir = workspace();
    let app = service(dir.path().join("signatures.yaml"), None).await;
    let response = served::call_service(&app, served::TestRequest::get().uri("/signatures.yaml").to_request()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(served::read_body(response).await, REGISTRY.as_bytes());

    let missing = service(dir.path().join("missing.yaml"), None).await;
    let response = served::call_service(
        &missing,
        served::TestRequest::get().uri("/signatures.yaml").to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[actix_rt::test]
async fn a_probe_page_is_served_from_the_root_given() {
    let dir = workspace();
    let app = service(dir.path().join("signatures.yaml"), Some(dir.path().to_owned())).await;
    let response = served::call_service(&app, served::TestRequest::get().uri("/").to_request()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(served::read_body(response).await, b"<p>probe page</p>".as_slice());

    let without = service(dir.path().join("signatures.yaml"), None).await;
    let response = served::call_service(&without, served::TestRequest::get().uri("/").to_request()).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[actix_rt::test]
async fn a_submission_is_added_then_found_present() {
    let dir = workspace();
    let path = dir.path().join("signatures.yaml");
    assert_eq!(
        register(path.clone(), NEW).await,
        (
            StatusCode::OK,
            true,
            "added; the registry now holds 2 entries".to_owned()
        )
    );
    assert_eq!(
        register(path, NEW).await,
        (StatusCode::OK, true, "already present: New".to_owned())
    );
}

#[actix_rt::test]
async fn a_body_that_is_not_a_submission_is_a_bad_request() {
    let dir = workspace();
    let (status, accepted, message) = register(dir.path().join("signatures.yaml"), "not json").await;
    assert_eq!((status, accepted), (StatusCode::BAD_REQUEST, false));
    assert!(message.starts_with("bad submission: "));
}

#[actix_rt::test]
async fn a_submission_not_sent_as_json_is_refused_unread() {
    let dir = workspace();
    let path = dir.path().join("signatures.yaml");
    let before = fs_err::read_to_string(&path).unwrap();
    // text/plain is what a page on another origin can post with no preflight.
    let (status, accepted, message) = register_as(path.clone(), NEW, "text/plain").await;
    assert_eq!((status, accepted), (StatusCode::UNSUPPORTED_MEDIA_TYPE, false));
    assert!(message.contains("application/json"));
    assert_eq!(fs_err::read_to_string(&path).unwrap(), before);
}

#[actix_rt::test]
async fn a_submission_the_registry_cannot_hold_is_a_conflict() {
    let dir = workspace();
    let unknown = NEW.replace("firefox", "netscape");
    let (status, accepted, message) = register(dir.path().join("signatures.yaml"), &unknown).await;
    assert_eq!((status, accepted), (StatusCode::CONFLICT, false));
    assert!(message.contains("netscape"));
}

#[actix_rt::test]
async fn a_registry_that_cannot_be_read_is_a_server_error() {
    let dir = workspace();
    let (status, accepted, _message) = register(dir.path().join("missing.yaml"), NEW).await;
    assert_eq!((status, accepted), (StatusCode::INTERNAL_SERVER_ERROR, false));
}

#[test]
fn plain_http_is_served_only_on_a_loopback_address() {
    use clap::Parser as _;
    use et_gpu_probe_server::{Args, StartError};

    let args = |extra: &[&str]| {
        let base = [
            "et-gpu-probe-server",
            "--registry",
            "r.yaml",
            "--cert",
            "c.pem",
            "--key",
            "k.pem",
        ];
        Args::parse_from(base.iter().chain(extra))
    };
    args(&["--http"]).check().unwrap();
    args(&["--http", "--bind", "::1"]).check().unwrap();
    args(&["--bind", "0.0.0.0"]).check().unwrap();
    let refused = args(&["--http", "--bind", "0.0.0.0"]).check().unwrap_err();
    assert!(matches!(refused, StartError::NotLoopback(_)));
    assert!(refused.to_string().contains("0.0.0.0"));
}

#[test]
fn a_certificate_covers_each_public_host_and_each_lan_address_with_its_nip_io_name() {
    let lan = [IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3))];
    assert_eq!(nip_io(lan[0]), "10-1-2-3.nip.io");
    assert_eq!(
        advertised_names(&["probe.example".to_owned()], &lan),
        ["probe.example", "10.1.2.3", "10-1-2-3.nip.io"]
    );
    assert_eq!(advertised_names(&[], &[]), Vec::<String>::new());
}
