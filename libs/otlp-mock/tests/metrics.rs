//! Exercises the mock's `/metrics` endpoint and `flatten_metrics` across every OTLP metric shape:
//! `Sum`, `Gauge`, `Histogram`, and a data-less metric, decoded from both JSON and protobuf bodies, plus a
//! non-string `service.name` and a malformed body. This is the direct-injection counterpart to the end-to-end
//! `et-ws-test-server` `OTel` test, which only drives the integer-counter path the hub actually emits.
#![cfg(test)]

use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::metrics::v1::{
    Gauge, Histogram, HistogramDataPoint, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, Sum, metric::Data,
    number_data_point,
};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message as _;

fn int_point(value: i64) -> NumberDataPoint {
    NumberDataPoint {
        value: Some(number_data_point::Value::AsInt(value)),
        ..Default::default()
    }
}

fn metric(name: &str, data: Option<Data>) -> Metric {
    Metric {
        name: name.to_owned(),
        unit: "1".to_owned(),
        data,
        ..Default::default()
    }
}

/// One resource carrying a non-string `service.name` (so the `StringValue` guard falls through to the empty
/// default) and one metric of every `Data` variant the flattener branches on, including a data-less one.
#[expect(
    clippy::single_call_fn,
    reason = "distinct fixture builder for the metric-shape matrix; kept separate"
)]
fn sample_request() -> ExportMetricsServiceRequest {
    let resource = Resource {
        attributes: vec![KeyValue {
            key: "service.name".to_owned(),
            // A non-string value: the flattener's `StringValue` binding must fail and yield the empty default.
            value: Some(AnyValue {
                value: Some(any_value::Value::IntValue(7)),
            }),
        }],
        ..Default::default()
    };
    let metrics = vec![
        metric(
            "sum.metric",
            Some(Data::Sum(Sum {
                data_points: vec![int_point(3), int_point(4)],
                ..Default::default()
            })),
        ),
        metric(
            "gauge.metric",
            Some(Data::Gauge(Gauge {
                data_points: vec![int_point(9)],
            })),
        ),
        metric(
            "hist.metric",
            Some(Data::Histogram(Histogram {
                data_points: vec![HistogramDataPoint::default(), HistogramDataPoint::default()],
                ..Default::default()
            })),
        ),
        metric("none.metric", None),
    ];
    ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: Some(resource),
            scope_metrics: vec![ScopeMetrics {
                metrics,
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

#[tokio::test]
async fn metrics_endpoint_decodes_json_protobuf_and_flattens_every_shape() {
    let mock = int_otlp_mock::start();
    let url = format!("{}/metrics", mock.collector_url());
    let client = reqwest::Client::new();
    let request = sample_request();

    // JSON body -- the encoding `et-otlp`'s JSON protocol uses; drives handle_metrics' serde_json branch.
    let json_resp = client
        .post(&url)
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&request).unwrap())
        .send()
        .await
        .unwrap();
    assert!(json_resp.status().is_success(), "JSON /metrics POST should succeed");

    // Protobuf body -- what a real OTLP relay sends; drives handle_metrics' prost branch.
    let mut proto_body = Vec::new();
    request.encode(&mut proto_body).unwrap();
    let proto_resp = client
        .post(&url)
        .header("content-type", "application/x-protobuf")
        .body(proto_body)
        .send()
        .await
        .unwrap();
    assert!(
        proto_resp.status().is_success(),
        "protobuf /metrics POST should succeed"
    );

    // Malformed body -- neither JSON nor protobuf decodes, so the handler must answer 400.
    let bad_resp = client
        .post(&url)
        .header("content-type", "application/json")
        .body(b"this is not a metrics payload".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(bad_resp.status().as_u16(), 400, "undecodable body must be rejected");

    // Both good posts landed, so every metric appears twice.
    let flat = mock.flatten_metrics();
    let count = |name: &str| flat.iter().filter(|rec| rec.name == name).count();
    assert_eq!(count("sum.metric"), 2);
    assert_eq!(count("gauge.metric"), 2);
    assert_eq!(count("hist.metric"), 2);
    assert_eq!(count("none.metric"), 2);

    let sum = flat.iter().find(|rec| rec.name == "sum.metric").unwrap();
    assert_eq!(sum.value, 7, "Sum sums its integer data points");
    assert_eq!(sum.data_points, 2);
    // The non-string service.name fell through to the empty default rather than being captured.
    assert_eq!(sum.service_name, "", "non-string service.name yields the empty default");

    let gauge = flat.iter().find(|rec| rec.name == "gauge.metric").unwrap();
    assert_eq!(gauge.value, 9, "Gauge sums its integer data points");

    let hist = flat.iter().find(|rec| rec.name == "hist.metric").unwrap();
    assert_eq!(hist.value, 0, "Histogram contributes no summed value");
    assert_eq!(hist.data_points, 2, "Histogram reports its data-point count");

    let none = flat.iter().find(|rec| rec.name == "none.metric").unwrap();
    assert_eq!(
        (none.value, none.data_points),
        (0, 0),
        "a data-less metric flattens to zero"
    );
}
