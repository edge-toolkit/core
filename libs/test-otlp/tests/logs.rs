//! Exercises log capture in both OTLP encodings.
//! `mock-collector` fixes a server's encoding at construction rather than sniffing `Content-Type`, so each
//! encoding gets its own server here; both must accept the same payload and surface it identically.
#![cfg(test)]

use et_test_otlp::{Protocol, ServerHandle};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message as _;

fn string_value(value: &str) -> AnyValue {
    AnyValue {
        value: Some(any_value::Value::StringValue(value.to_owned())),
    }
}

fn sample_request() -> ExportLogsServiceRequest {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![KeyValue {
                    key: "service.name".to_owned(),
                    value: Some(string_value("logs-test")),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            scope_logs: vec![ScopeLogs {
                log_records: vec![LogRecord {
                    body: Some(string_value("hello from the hub")),
                    severity_text: "INFO".to_owned(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

/// POST `body` to the handle's `/v1/logs` under `content_type`, returning the HTTP status.
async fn post_logs(handle: &ServerHandle, content_type: &str, body: Vec<u8>) -> u16 {
    reqwest::Client::new()
        .post(format!("{}/logs", et_test_otlp::collector_url(handle)))
        .header("content-type", content_type)
        .body(body)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

#[tokio::test]
async fn json_server_accepts_json_log_payloads() {
    let mock = et_test_otlp::start(Protocol::HttpJson).await;
    let request = sample_request();

    let status = post_logs(&mock, "application/json", serde_json::to_vec(&request).unwrap()).await;
    assert_eq!(status, 200, "JSON /logs POST should succeed");

    // Malformed body -- nothing decodes, so the handler must reject it rather than record an empty entry.
    let bad = post_logs(&mock, "application/json", b"this is not a logs payload".to_vec()).await;
    assert_eq!(bad, 400, "undecodable body must be rejected");

    mock.with_collector(|collector| {
        assert_eq!(collector.log_count(), 1, "only the well-formed post is recorded");
        collector
            .expect_log_with_body("hello from the hub")
            .with_severity_text("INFO")
            .assert_exists();
    })
    .await;
}

#[tokio::test]
async fn protobuf_server_accepts_protobuf_log_payloads() {
    let mock = et_test_otlp::start(Protocol::HttpBinary).await;
    let request = sample_request();

    let mut body = Vec::new();
    request.encode(&mut body).unwrap();
    let status = post_logs(&mock, "application/x-protobuf", body).await;
    assert_eq!(status, 200, "protobuf /logs POST should succeed");

    let bad = post_logs(&mock, "application/x-protobuf", b"not protobuf".to_vec()).await;
    assert_eq!(bad, 400, "undecodable body must be rejected");

    mock.with_collector(|collector| {
        assert_eq!(collector.log_count(), 1, "only the well-formed post is recorded");
        collector
            .expect_log_with_body("hello from the hub")
            .with_severity_text("INFO")
            .assert_exists();
    })
    .await;
}
