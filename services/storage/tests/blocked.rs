//! A client address the hub has blocked loses every storage route at once, while other addresses keep them.
#![cfg(test)]

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::pin::Pin;

use actix_http::Request;
use actix_web::dev::{Payload, Service, ServiceResponse};
use actix_web::error::PayloadError;
use actix_web::http::{Method, StatusCode};
use actix_web::web::Bytes;
use actix_web::{App, test, web};
use edge_toolkit::ws::AgentConnectionState;
use edge_toolkit::ws_server::{AgentRecord, AgentRegistry};
use et_storage_service::{StorageConfig, configure};
use futures_util::{Stream, stream};
use tempfile::TempDir;

const BLOCKED: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 7));
const ALLOWED: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 8));

fn request(method: Method, from: IpAddr) -> test::TestRequest {
    test::TestRequest::default()
        .method(method)
        .uri("/storage/agent-1/file.txt")
        .peer_addr(SocketAddr::new(from, 40000))
        .set_payload(b"hello".as_ref())
}

/// The storage routes over a fresh local store, with the registry they answer for and the store's directory.
///
/// The registry holds `agent-1`, connected from `ALLOWED`.
#[expect(
    clippy::future_not_send,
    reason = "actix-web test services are Rc-backed and !Send; runs on actix's single-threaded test rt"
)]
async fn storage_app() -> (
    AgentRegistry<()>,
    impl Service<Request, Response = ServiceResponse, Error = actix_web::Error>,
    TempDir,
) {
    let agents = BTreeMap::from([(
        "agent-1".to_owned(),
        AgentRecord::new(AgentConnectionState::Connected, ALLOWED, Some(())),
    )]);
    let registry = AgentRegistry::from_agents(agents);
    let root = tempfile::tempdir().unwrap();
    let config = StorageConfig::local(root.path());
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(registry.clone()))
            .configure(|cfg| configure::<()>(cfg, &config)),
    )
    .await;
    (registry, app, root)
}

#[actix_rt::test]
async fn put_get_and_head_refuse_a_blocked_address() {
    let (registry, app, _root) = storage_app().await;

    let stored = test::call_service(&app, request(Method::PUT, ALLOWED).to_request()).await;
    assert_eq!(stored.status(), StatusCode::OK);
    assert!(registry.blocklist.block(BLOCKED));

    for method in [Method::PUT, Method::GET, Method::HEAD] {
        let refused = test::call_service(&app, request(method.clone(), BLOCKED).to_request()).await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
        let served = test::call_service(&app, request(method, ALLOWED).to_request()).await;
        assert_eq!(served.status(), StatusCode::OK);
    }
}

#[actix_rt::test]
async fn an_upload_is_refused_when_its_sender_is_blocked_while_the_body_streams() {
    let (registry, app, root) = storage_app().await;

    let blocklist = registry.blocklist.clone();
    let body: Pin<Box<dyn Stream<Item = Result<Bytes, PayloadError>>>> = Box::pin(stream::once(async move {
        assert!(blocklist.block(ALLOWED));
        Ok(Bytes::from_static(b"hello"))
    }));
    let (upload, _) = request(Method::PUT, ALLOWED)
        .to_request()
        .replace_payload(Payload::from(body));
    let refused = test::call_service(&app, upload).await;
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert!(!root.path().join("agent-1").join("file.txt").exists());
}
