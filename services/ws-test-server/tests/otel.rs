//! End-to-end `OTel` coverage for the ws hub across all three signals -- traces,
//! logs, and metrics -- captured by an in-process OTLP mock.
//!
//! Boots the hub via `et_ws_test_server`, points the process's OTLP exporters at
//! the mock (installed as the global tracer/logger/meter providers by
//! `et_otlp::init`), drives a websocket client so the hub emits a `ws.connect`
//! span, connection `info!` logs, and the connection/message metrics, then
//! flushes on `OtelHandles::shutdown` and asserts every signal arrived.
//!
//! The collector is the third-party `mock-collector` crate, reached through `et-test-otlp`'s helpers.
#![cfg(test)]

use std::time::Duration;

use edge_toolkit::config::{OtlpConfig, OtlpProtocol};
use et_test_otlp::Protocol;
use et_ws_test_server::connect_agent;
use futures_util::SinkExt as _;
use tokio_tungstenite::tungstenite::Message;

/// How long to wait for a signal to reach the collector after `shutdown` flushes the exporters.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(15);

// `multi_thread` is load-bearing.
// The collector's server task is spawned onto the *caller's* runtime, and `OtelHandles::shutdown` is a
// blocking flush. On the default current-thread runtime that flush occupies the only worker, the server task
// never gets polled, and every export fails with
// `BatchSpanProcessor.ExportError ... HTTP export failed: network error`.
#[tokio::test(flavor = "multi_thread")]
async fn hub_emits_traces_logs_and_metrics() {
    let mock = et_test_otlp::start(Protocol::HttpJson).await;

    // Point the in-process OTLP exporters at the mock and install the global providers.
    let mut otlp = OtlpConfig::default();
    otlp.collector_url = et_test_otlp::collector_url(&mock);
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
    mock.wait_until(
        |collector| collector.spans().iter().any(|span| span.span().name == "ws.connect"),
        FLUSH_TIMEOUT,
    )
    .await
    .unwrap();
    mock.with_collector(|collector| {
        collector
            .expect_span_with_name("ws.connect")
            .with_resource_attributes([("service.name", "et-ws-test")])
            .assert_exists();
    })
    .await;

    // Logs: the hub's connection `info!` lines reached the collector.
    mock.wait_for_logs(1, FLUSH_TIMEOUT).await.unwrap();

    // Metrics: the inbound-message counter (>= 1 after the connect + relayed frame).
    mock.wait_until(
        |collector| {
            collector
                .metrics()
                .iter()
                .any(|metric| metric.metric().name == "et_ws.messages.received")
        },
        FLUSH_TIMEOUT,
    )
    .await
    .unwrap();
    mock.with_collector(|collector| {
        collector
            .expect_metric_with_name("et_ws.messages.received")
            .with_value_gte(1_i64)
            .assert_exists();
        collector
            .expect_metric_with_name("et_ws.connections.active")
            .assert_exists();
    })
    .await;
}
