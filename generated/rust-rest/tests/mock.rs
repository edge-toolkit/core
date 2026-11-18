//! Covers the generated `httpmock` extension: the client's own calls are answered by mocks built from the spec.
#![cfg(test)]

use et_rest_client::Client;
use et_rest_client::mock::MockServerExt as _;
use et_rest_client::types::{BuildInfo, HealthResponse};
use httpmock::MockServer;

#[tokio::test]
async fn health_is_answered_by_its_typed_mock() {
    let server = MockServer::start_async().await;
    let healthy = HealthResponse {
        service: "ws-server".to_owned(),
        status: "healthy".to_owned(),
    };
    let mock = server.health(|when, then| {
        let _when = when;
        let _then = then.ok(&healthy);
    });

    let response = Client::new(&server.base_url())
        .health()
        .await
        .expect("the mock answers /health");

    assert_eq!(response.into_inner().status, "healthy");
    mock.assert_async().await;
}

#[tokio::test]
async fn path_parameter_matches_only_its_literal_value() {
    let server = MockServer::start_async().await;
    let mock = server.block_ip(|when, then| {
        let _when = when.ip("127.0.0.1");
        let _then = then.no_content();
    });
    let client = Client::new(&server.base_url());

    assert!(client.block_ip("127x0x0x1").await.is_err());
    assert!(client.block_ip("127.0.0.1").await.is_ok());
    mock.assert_calls_async(1).await;
}

#[tokio::test]
async fn info_is_answered_by_its_typed_mock() {
    let server = MockServer::start_async().await;
    let build = BuildInfo {
        features: vec!["default".to_owned()],
        name: "et-ws-server".to_owned(),
        profile: "debug".to_owned(),
        rustc: "rustc 1.100.0".to_owned(),
        target: "x86_64-unknown-linux-gnu".to_owned(),
        version: "0.3.0".to_owned(),
    };
    let mock = server.info(|when, then| {
        let _when = when;
        let _then = then.ok(&build);
    });

    let response = Client::new(&server.base_url())
        .info()
        .await
        .expect("the mock answers /info");

    assert_eq!(response.into_inner().name, "et-ws-server");
    mock.assert_async().await;
}
