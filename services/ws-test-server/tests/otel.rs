//! End-to-end `OTel` coverage for the ws hub across all three signals -- traces,
//! logs, and metrics -- captured by the in-process OTLP mock.
//!
//! Boots the hub via `et_ws_test_server`, points the process's OTLP exporters at
//! the mock (installed as the global tracer/logger/meter providers by
//! `et_otlp::init`), drives a websocket client so the hub emits a `ws.connect`
//! span, connection `info!` logs, and the connection/message metrics, then
//! flushes on `OtelHandles::shutdown` and asserts every signal arrived.
#![cfg(test)]

use edge_toolkit::config::{OtlpConfig, OtlpProtocol};
use et_ws_test_server::connect_agent;
use futures_util::SinkExt as _;
use retry::delay::Fixed;
use retry::retry;
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn hub_emits_traces_logs_and_metrics() {
    let mock = int_otlp_mock::start();

    // Point the in-process OTLP exporters at the mock and install the global providers.
    let mut otlp = OtlpConfig::default();
    otlp.collector_url = mock.collector_url().to_owned();
    otlp.protocol = OtlpProtocol::JSON;
    otlp.service_label = "et-ws-test".to_string();
    otlp.auth = None;
    let handles = et_otlp::init(&otlp).unwrap();

    let server = et_ws_test_server::start();

    // Drive one client through connect + a relayed frame, then disconnect, so the hub emits a `ws.connect`
    // span, connection `info!` logs, and the connection/message metrics.
    {
        let (mut stream, _agent_id) = connect_agent(&server.ws_url).await;
        stream.send(Message::text("hello hub")).await.unwrap();
        stream.close(None).await.unwrap();
    }

    // Flush batched spans/logs/metrics to the mock.
    handles.shutdown();

    // Traces: a `ws.connect` span tagged with our service name.
    retry(Fixed::from_millis(200).take(75), || {
        mock.flatten_spans()
            .iter()
            .any(|span| span.service_name == "et-ws-test" && span.name == "ws.connect")
            .then_some(())
            .ok_or(())
    })
    .unwrap();

    // Logs: the hub's connection `info!` lines reached the collector.
    assert!(!mock.logs().is_empty(), "expected hub info! logs at the collector");

    // Metrics: the inbound-message counter (>= 1 after the connect + relayed frame).
    let metrics = retry(Fixed::from_millis(200).take(75), || {
        let metrics = mock.flatten_metrics();
        metrics
            .iter()
            .any(|metric| metric.name == "et_ws.messages.received" && metric.value >= 1)
            .then_some(metrics)
            .ok_or(())
    })
    .unwrap();
    assert!(
        metrics.iter().any(|metric| metric.name == "et_ws.connections.active"),
        "expected the et_ws.connections.active gauge, got {metrics:?}"
    );
}
