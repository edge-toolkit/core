//! In-process mock OTLP/HTTP collector (protobuf or JSON).
//!
//! Used by integration tests to verify trace-context propagation across
//! processes: both the system-under-test (ws-server, ws-wasi-runner, ...)
//! point their OTLP exporters at this collector, then the test reads the
//! captured spans back to assert that trace ids match.
//!
//! This is **not** a real OTLP implementation -- it just buffers payloads.
//! Endpoints match the URL shape `et-otlp::init` produces:
//!
//!   - `POST <collector_url>/traces` -- accepts OTLP/HTTP in either encoding,
//!     chosen by `Content-Type`: `application/x-protobuf` (what a real relay
//!     such as Vector's opentelemetry sink emits) or JSON (what `et-otlp` sends
//!     with `OTLP_PROTOCOL=JSON`). Both decode to the same `ExportTraceServiceRequest`.
//!   - `POST <collector_url>/logs` -- OTLP/HTTP-JSON log payloads.
//!   - `POST <collector_url>/metrics` -- OTLP/HTTP metric payloads (protobuf or JSON, like `/traces`).
//!
//! Read captured spans back via [`OtlpMock::flatten_spans`], logs via
//! [`OtlpMock::logs`], and metrics via [`OtlpMock::flatten_metrics`].
#![expect(
    clippy::unwrap_used,
    clippy::panic,
    clippy::exhaustive_structs,
    reason = "test mock; bind/poison/startup failures fail fast; actix #[post] marker structs can't be annotated"
)]

use std::sync::{Arc, Mutex};

use actix_web::http::header::ContentType;
use actix_web::{App, HttpResponse, HttpServer, post, web};
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value;
use opentelemetry_proto::tonic::metrics::v1::{metric::Data, number_data_point};
use prost::Message as _;
use serde_json::Value;

#[derive(Default)]
struct Captured {
    traces: Mutex<Vec<ExportTraceServiceRequest>>,
    logs: Mutex<Vec<Value>>,
    metrics: Mutex<Vec<ExportMetricsServiceRequest>>,
}

/// Handle to a running mock collector.
///
/// The server is shut down when this struct is dropped (the actix runtime is
/// owned by the spawned thread -- when our struct goes out of scope, the
/// spawned thread's tokio runtime stays alive but the handle pointing at it
/// is dropped, which is fine for test scope).
pub struct OtlpMock {
    collector_url: String,
    captured: Arc<Captured>,
}

impl OtlpMock {
    /// Pass this to `OTLP_COLLECTOR_URL` in env so OTLP exporters target the mock.
    ///
    /// Trace endpoint is `<collector_url>/traces`; logs is `<collector_url>/logs` -- matches
    /// `et_otlp::init`'s URL convention.
    #[must_use]
    pub fn collector_url(&self) -> &str {
        &self.collector_url
    }

    /// Snapshot the log payloads received so far.
    #[must_use]
    pub fn logs(&self) -> Vec<Value> {
        self.captured.logs.lock().unwrap().clone()
    }

    /// Walk every span across every captured request, pairing each with its service name.
    ///
    /// The name is the parent `Resource`'s `service.name` attribute (so the test can group spans
    /// by service). Trace/span ids are lowercase-hex-encoded from the decoded bytes -- compare
    /// against [`to_hex`] of the raw ids you sent.
    #[must_use]
    pub fn flatten_spans(&self) -> Vec<FlatSpan> {
        let mut out = Vec::new();
        for req in self.captured.traces.lock().unwrap().iter() {
            for resource_span in &req.resource_spans {
                let service_name = resource_span
                    .resource
                    .as_ref()
                    .and_then(|resource| {
                        resource
                            .attributes
                            .iter()
                            .filter(|attr| attr.key == "service.name")
                            .find_map(|attr| {
                                let any_value::Value::StringValue(value) = attr.value.as_ref()?.value.as_ref()? else {
                                    return None;
                                };
                                Some(value.clone())
                            })
                    })
                    .unwrap_or_default();
                for scope_span in &resource_span.scope_spans {
                    for span in &scope_span.spans {
                        out.push(FlatSpan {
                            service_name: service_name.clone(),
                            trace_id: to_hex(&span.trace_id),
                            span_id: to_hex(&span.span_id),
                            parent_span_id: to_hex(&span.parent_span_id),
                            name: span.name.clone(),
                        });
                    }
                }
            }
        }
        out
    }

    /// Walk every metric across every captured request, pairing each with its `Resource`'s `service.name`.
    /// `value` sums the numeric (Sum/Gauge) data points -- so a monotonic counter reads as its running total --
    /// while `data_points` counts them (histogram points are counted but don't contribute to `value`).
    #[must_use]
    pub fn flatten_metrics(&self) -> Vec<FlatMetric> {
        let mut out = Vec::new();
        for req in self.captured.metrics.lock().unwrap().iter() {
            for resource_metric in &req.resource_metrics {
                let service_name = resource_metric
                    .resource
                    .as_ref()
                    .and_then(|resource| {
                        resource
                            .attributes
                            .iter()
                            .filter(|attr| attr.key == "service.name")
                            .find_map(|attr| {
                                let any_value::Value::StringValue(value) = attr.value.as_ref()?.value.as_ref()? else {
                                    return None;
                                };
                                Some(value.clone())
                            })
                    })
                    .unwrap_or_default();
                for scope_metric in &resource_metric.scope_metrics {
                    for metric in &scope_metric.metrics {
                        let (value, data_points) = match &metric.data {
                            Some(Data::Sum(sum)) => sum_number_points(&sum.data_points),
                            Some(Data::Gauge(gauge)) => sum_number_points(&gauge.data_points),
                            Some(Data::Histogram(histogram)) => (0, histogram.data_points.len()),
                            _ => (0, 0),
                        };
                        out.push(FlatMetric {
                            service_name: service_name.clone(),
                            name: metric.name.clone(),
                            unit: metric.unit.clone(),
                            value,
                            data_points,
                        });
                    }
                }
            }
        }
        out
    }
}

/// Sum the integer OTLP data points into `(total, count)`; float data points don't contribute to `total`.
fn sum_number_points(points: &[opentelemetry_proto::tonic::metrics::v1::NumberDataPoint]) -> (i64, usize) {
    let mut total: i64 = 0;
    for point in points {
        if let Some(number_data_point::Value::AsInt(value)) = point.value {
            total = total.saturating_add(value);
        }
    }
    (total, points.len())
}

/// Lowercase-hex-encode bytes -- used for the trace/span ids in [`FlatSpan`].
#[must_use]
pub fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::default();
    for byte in bytes {
        // Writing to a String is infallible; unwrap is allowed crate-wide.
        write!(out, "{byte:02x}").unwrap();
    }
    out
}

/// Flattened span view for assertions.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct FlatSpan {
    pub service_name: String,
    /// Lowercase-hex-encoded 16-byte trace id.
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: String,
    pub name: String,
}

/// Flattened metric view for assertions.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct FlatMetric {
    pub service_name: String,
    pub name: String,
    pub unit: String,
    /// Sum of the metric's integer (Sum/Gauge) data points -- a monotonic `u64`/`i64` counter's running total.
    /// Floating-point data points are ignored (the metrics emitted here are integer counters).
    pub value: i64,
    /// Number of data points seen for this metric (includes histogram points).
    pub data_points: usize,
}

#[expect(
    clippy::single_call_fn,
    reason = "actix-web route handler; registered via the #[post] macro"
)]
#[post("/traces")]
async fn handle_traces(
    state: web::Data<Arc<Captured>>,
    content_type: web::Header<ContentType>,
    body: web::Bytes,
) -> HttpResponse {
    // `web::Header` (not `HttpRequest`) keeps the handler's future `Send`.
    let is_protobuf = content_type.0.subtype().as_str().contains("protobuf");
    // Decode either encoding to the one `ExportTraceServiceRequest` shape.
    // Slice-decode so prost's `Buf` bound is met by `&[u8]`, avoiding any
    // `bytes` crate version identity concern with `web::Bytes`.
    let decoded = if is_protobuf {
        ExportTraceServiceRequest::decode(body.as_ref()).ok()
    } else {
        serde_json::from_slice::<ExportTraceServiceRequest>(&body).ok()
    };
    let Some(trace_request) = decoded else {
        return HttpResponse::BadRequest().finish();
    };
    state.traces.lock().unwrap().push(trace_request);
    // OTLP success is an empty `ExportTraceServiceResponse`: `{}` in JSON, no
    // bytes in protobuf. Echo the request's encoding back.
    if is_protobuf {
        HttpResponse::Ok()
            .content_type("application/x-protobuf")
            .body(Vec::new())
    } else {
        HttpResponse::Ok().content_type("application/json").body("{}")
    }
}

#[expect(
    clippy::single_call_fn,
    reason = "actix-web route handler; registered via the #[post] macro"
)]
#[post("/logs")]
async fn handle_logs(state: web::Data<Arc<Captured>>, body: web::Json<Value>) -> HttpResponse {
    state.logs.lock().unwrap().push(body.into_inner());
    HttpResponse::Ok().content_type("application/json").body("{}")
}

#[expect(
    clippy::single_call_fn,
    reason = "actix-web route handler; registered via the #[post] macro"
)]
#[post("/metrics")]
async fn handle_metrics(
    state: web::Data<Arc<Captured>>,
    content_type: web::Header<ContentType>,
    body: web::Bytes,
) -> HttpResponse {
    // Same dual-encoding decode as `/traces`: protobuf from a real relay, JSON from `et-otlp`'s JSON protocol.
    let is_protobuf = content_type.0.subtype().as_str().contains("protobuf");
    let decoded = if is_protobuf {
        ExportMetricsServiceRequest::decode(body.as_ref()).ok()
    } else {
        serde_json::from_slice::<ExportMetricsServiceRequest>(&body).ok()
    };
    let Some(metrics_request) = decoded else {
        return HttpResponse::BadRequest().finish();
    };
    state.metrics.lock().unwrap().push(metrics_request);
    if is_protobuf {
        HttpResponse::Ok()
            .content_type("application/x-protobuf")
            .body(Vec::new())
    } else {
        HttpResponse::Ok().content_type("application/json").body("{}")
    }
}

/// Start the mock on a free port and return its handle.
///
/// The HTTP server runs on its own thread + actix runtime; the test's
/// runtime is untouched.
#[must_use]
pub fn start() -> OtlpMock {
    start_on(et_test_helpers::reserve_port())
}

/// Start the mock on a caller-chosen port (which must be free).
///
/// Store-and-forward tests reserve a free port, point a relay's sink at it
/// while nothing is listening, then call this to bring the collector up on
/// the same port -- proving the relay's buffered data gets delivered once the
/// backend appears.
#[must_use]
pub fn start_on(port: u16) -> OtlpMock {
    let captured = Arc::new(Captured::default());
    let captured_for_server = Arc::clone(&captured);
    let addr = format!("127.0.0.1:{port}");

    let _join = std::thread::spawn(move || {
        actix_rt::System::new().block_on(async move {
            let data = web::Data::new(captured_for_server);
            HttpServer::new(move || {
                App::new()
                    .app_data(data.clone())
                    .app_data(web::JsonConfig::default().limit(64 * 1024 * 1024))
                    .app_data(web::PayloadConfig::new(64 * 1024 * 1024))
                    .service(handle_traces)
                    .service(handle_logs)
                    .service(handle_metrics)
            })
            .bind(&addr)
            .unwrap()
            .run()
            .await
            .unwrap();
        });
    });

    // Wait for the server to start accepting connections so the caller can
    // immediately point exporters at it.
    for _ in 0_u32..50 {
        if std::net::TcpStream::connect(format!("127.0.0.1:{port}")).is_ok() {
            return OtlpMock {
                collector_url: format!("http://127.0.0.1:{port}"),
                captured,
            };
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("otlp-mock did not start within 5 seconds on port {port}");
}
