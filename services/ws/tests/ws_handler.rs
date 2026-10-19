//! Covers `ws_handler`'s client address: an upgrade from a socket peer is accepted, and a request with no peer
//! address -- only possible on a non-TCP listener, which the server never binds -- is refused.
#![cfg(test)]

use std::net::SocketAddr;

use actix_web::http::{StatusCode, header};
use actix_web::{App, test, web};
use et_ws_service::{WsAgentRegistry, WsConfig, configure};

/// The handshake nonce from RFC 6455's opening-handshake example: base64 of the 16 bytes `the sample nonce`.
const RFC_6455_SAMPLE_NONCE: &str = "dGhlIHNhbXBsZSBub25jZQ==";

fn upgrade_request() -> test::TestRequest {
    test::TestRequest::get()
        .uri("/ws")
        .insert_header((header::UPGRADE, "websocket"))
        .insert_header((header::CONNECTION, "upgrade"))
        .insert_header((header::SEC_WEBSOCKET_VERSION, "13"))
        .insert_header((header::SEC_WEBSOCKET_KEY, RFC_6455_SAMPLE_NONCE))
}

#[expect(
    clippy::future_not_send,
    reason = "actix-web TestRequest and test service are Rc-backed and !Send; runs on actix's single-threaded test rt"
)]
async fn status_of(req: test::TestRequest) -> StatusCode {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(WsAgentRegistry::default()))
            .configure(|cfg| configure(cfg, &WsConfig::default())),
    )
    .await;
    test::call_service(&app, req.to_request()).await.status()
}

#[actix_rt::test]
async fn an_upgrade_from_a_socket_peer_switches_protocols() {
    let peer: SocketAddr = "192.0.2.7:40000".parse().unwrap();

    assert_eq!(
        status_of(upgrade_request().peer_addr(peer)).await,
        StatusCode::SWITCHING_PROTOCOLS
    );
}

#[actix_rt::test]
async fn an_upgrade_without_a_peer_address_is_refused() {
    assert_eq!(status_of(upgrade_request()).await, StatusCode::INTERNAL_SERVER_ERROR);
}
