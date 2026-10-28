//! Covers the admin API on a local hub: its token's parsing, the auth gate in front of `PUT /blocked-ips/{ip}` and
//! `DELETE /agents/{agent_id}`, what a block and an eviction refuse, and how each failure maps to a status.
#![cfg(test)]

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use actix_web::dev::{Service, ServiceResponse};
use actix_web::http::{StatusCode, header};
use actix_web::{App, ResponseError as _, test, web};
use edge_toolkit::ws_server::{MessageRetention, RegistryError};
use et_ws_service::admin::{AdminError, AdminToken, EvictedAgent};
use et_ws_service::{BackplaneError, WsAgentRegistry, WsConfig, configure};

const TOKEN: &str = "s3cret";
const AGENT_IP: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 7));
const OTHER_IP: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 8));

/// The handshake nonce from RFC 6455's opening-handshake example: base64 of the 16 bytes `the sample nonce`.
const RFC_6455_SAMPLE_NONCE: &str = "dGhlIHNhbXBsZSBub25jZQ==";

fn config_with(token: AdminToken) -> WsConfig {
    let mut config = WsConfig::default();
    config.admin_token = token;
    config
}

fn enabled() -> AdminToken {
    AdminToken::try_from(TOKEN.to_owned()).unwrap()
}

/// A registry holding one agent, `agent-1`, last seen at [`AGENT_IP`].
fn registry_with_agent() -> WsAgentRegistry {
    let registry = WsAgentRegistry::default();
    let (session, _outbox) = tokio::sync::mpsc::unbounded_channel();
    let _connected = registry.connect_agent(Some("agent-1".to_owned()), "unused".to_owned(), AGENT_IP, session);
    registry
}

#[expect(
    clippy::future_not_send,
    reason = "actix-web test services are Rc-backed and !Send; runs on actix's single-threaded test rt"
)]
async fn app(
    registry: WsAgentRegistry,
    config: WsConfig,
) -> impl Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error> {
    test::init_service(
        App::new()
            .app_data(web::Data::new(registry))
            .configure(|cfg| configure(cfg, &config)),
    )
    .await
}

/// Assert [`AGENT_IP`] is on `registry`'s blocklist and `app` refuses its upgrade, while it accepts [`OTHER_IP`]'s.
#[expect(
    clippy::future_not_send,
    reason = "actix-web test services are Rc-backed and !Send; runs on actix's single-threaded test rt"
)]
async fn assert_only_the_agents_address_is_refused(
    registry: &WsAgentRegistry,
    app: &impl Service<actix_http::Request, Response = ServiceResponse, Error = actix_web::Error>,
) {
    assert!(registry.blocklist.is_blocked(AGENT_IP));
    let refused = test::call_service(app, upgrade_from(AGENT_IP).to_request()).await;
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    let accepted = test::call_service(app, upgrade_from(OTHER_IP).to_request()).await;
    assert_eq!(accepted.status(), StatusCode::SWITCHING_PROTOCOLS);
}

fn with_credential(request: test::TestRequest, credential: Option<&str>) -> test::TestRequest {
    match credential {
        Some(credential) => request.insert_header((header::AUTHORIZATION, credential)),
        None => request,
    }
}

fn evict(credential: Option<&str>) -> test::TestRequest {
    with_credential(test::TestRequest::delete().uri("/agents/agent-1"), credential)
}

fn block(ip: &str, credential: Option<&str>) -> test::TestRequest {
    with_credential(test::TestRequest::put().uri(&format!("/blocked-ips/{ip}")), credential)
}

fn upgrade_from(ip: IpAddr) -> test::TestRequest {
    test::TestRequest::get()
        .uri("/ws")
        .peer_addr(SocketAddr::new(ip, 40000))
        .insert_header((header::UPGRADE, "websocket"))
        .insert_header((header::CONNECTION, "upgrade"))
        .insert_header((header::SEC_WEBSOCKET_VERSION, "13"))
        .insert_header((header::SEC_WEBSOCKET_KEY, RFC_6455_SAMPLE_NONCE))
}

#[actix_rt::test]
async fn the_token_is_disabled_by_default_and_an_empty_one_is_rejected() {
    assert!(matches!(WsConfig::default().admin_token, AdminToken::Disabled));
    assert!(matches!(enabled(), AdminToken::Token(_)));
    let error = serde_json::from_str::<AdminToken>(r#""""#).unwrap_err();
    assert!(error.to_string().contains("WS_ADMIN_TOKEN is set but empty"));
    assert!(!format!("{:?}", enabled()).contains(TOKEN));
}

#[actix_rt::test]
async fn a_disabled_admin_api_refuses_even_the_right_token() {
    let app = app(registry_with_agent(), WsConfig::default()).await;
    let response = test::call_service(&app, evict(Some("Bearer s3cret")).to_request()).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[actix_rt::test]
async fn a_missing_malformed_or_wrong_credential_is_challenged() {
    let registry = registry_with_agent();
    let app = app(registry.clone(), config_with(enabled())).await;
    for credential in [
        None,
        Some("Basic s3cret"),
        Some("Bearer wrong"),
        Some("Bearer s3cret-and-more"),
    ] {
        let response = test::call_service(&app, evict(credential).to_request()).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers().get(header::WWW_AUTHENTICATE).unwrap(), "Bearer");
    }
    assert!(!registry.blocklist.is_blocked(AGENT_IP));
}

#[actix_rt::test]
async fn an_unknown_agent_is_not_found() {
    let app = app(WsAgentRegistry::default(), config_with(enabled())).await;
    let response = test::call_service(&app, evict(Some("Bearer s3cret")).to_request()).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[actix_rt::test]
async fn an_eviction_blocks_the_agents_address_from_reconnecting() {
    let registry = registry_with_agent();
    let app = app(registry.clone(), config_with(enabled())).await;

    let evicted: EvictedAgent = test::call_and_read_body_json(&app, evict(Some("Bearer s3cret")).to_request()).await;
    assert_eq!(evicted.agent_id, "agent-1");
    assert_eq!(evicted.blocked_ip, AGENT_IP);
    assert_only_the_agents_address_is_refused(&registry, &app).await;
}

#[actix_rt::test]
async fn a_blocked_address_is_refused_and_blocking_it_again_succeeds() {
    let registry = WsAgentRegistry::default();
    let app = app(registry.clone(), config_with(enabled())).await;

    for _attempt in 0..2_u8 {
        let response = test::call_service(&app, block("192.0.2.7", Some("Bearer s3cret")).to_request()).await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }
    assert_only_the_agents_address_is_refused(&registry, &app).await;
}

#[actix_rt::test]
async fn a_block_needs_an_address_and_the_admin_token() {
    let registry = WsAgentRegistry::default();
    let hub = app(registry.clone(), config_with(enabled())).await;

    let malformed = test::call_service(&hub, block("not-an-ip", Some("Bearer s3cret")).to_request()).await;
    assert_eq!(malformed.status(), StatusCode::NOT_FOUND);
    let unauthorized = test::call_service(&hub, block("192.0.2.7", None).to_request()).await;
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let ipv6 = test::call_service(&hub, block("2001:db8::1", Some("Bearer s3cret")).to_request()).await;
    assert_eq!(ipv6.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        registry.blocklist.ips(),
        BTreeSet::from(["2001:db8::1".parse().unwrap()])
    );

    let disabled = app(WsAgentRegistry::default(), WsConfig::default()).await;
    let refused = test::call_service(&disabled, block("192.0.2.7", Some("Bearer s3cret")).to_request()).await;
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
}

#[actix_rt::test]
async fn an_eviction_drops_the_messages_the_agent_queued() {
    let registry = registry_with_agent();
    let (session, _outbox) = tokio::sync::mpsc::unbounded_channel();
    let _connected = registry.connect_agent(Some("agent-2".to_owned()), "unused".to_owned(), OTHER_IP, session);
    let _queued = registry.queue_direct(
        "m1".to_owned(),
        "agent-1",
        "agent-2",
        "2026-10-28T00:00:00Z".to_owned(),
        serde_json::Value::Null,
        MessageRetention::All,
    );
    let app = app(registry.clone(), config_with(enabled())).await;

    let response = test::call_service(&app, evict(Some("Bearer s3cret")).to_request()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(registry.pending_messages_for("agent-2").is_empty());
}

#[actix_rt::test]
async fn each_failure_maps_to_its_status() {
    let from_registry = AdminError::from(RegistryError::LockPoisoned);
    assert_eq!(from_registry.status_code(), StatusCode::INTERNAL_SERVER_ERROR);
    let from_backplane = AdminError::from(BackplaneError::NoPendingMessage("m".to_owned()));
    assert_eq!(from_backplane.status_code(), StatusCode::SERVICE_UNAVAILABLE);
    let unknown = AdminError::from(BackplaneError::UnknownAgent("a".to_owned()));
    assert_eq!(unknown.status_code(), StatusCode::NOT_FOUND);
    assert_eq!(unknown.to_string(), "unknown agent a");
}
