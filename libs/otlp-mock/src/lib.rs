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
//!
//! Read captured spans back via [`OtlpMock::flatten_spans`] and logs via
//! [`OtlpMock::logs`].
#![expect(
    clippy::unwrap_used,
    clippy::panic,
    clippy::exhaustive_structs,
    reason = "test mock; bind/poison/startup failures fail fast; actix #[post] marker structs can't be annotated"
)]

use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use actix_web::http::header::ContentType;
use actix_web::{App, HttpResponse, HttpServer, post, web};
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value;
use prost::Message as _;
use serde_json::Value;

#[derive(Default)]
struct Captured {
    traces: Mutex<Vec<ExportTraceServiceRequest>>,
    logs: Mutex<Vec<Value>>,
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
    /// Pass this to `OTLP_COLLECTOR_URL` in env so OTLP exporters target
    /// the mock. Trace endpoint is `<collector_url>/traces`; logs is
    /// `<collector_url>/logs` -- matches `et_otlp::init`'s URL convention.
    #[must_use]
    pub fn collector_url(&self) -> &str {
        &self.collector_url
    }

    /// Snapshot the log payloads received so far.
    #[must_use]
    pub fn logs(&self) -> Vec<Value> {
        self.captured.logs.lock().unwrap().clone()
    }

    /// Walk every span across every captured request, returning each span with
    /// its parent `Resource`'s `service.name` attribute (so the test can group
    /// spans by service). Trace/span ids are lowercase-hex-encoded from the
    /// decoded bytes -- compare against [`to_hex`] of the raw ids you sent.
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
}

/// Lowercase-hex-encode bytes -- used for the trace/span ids in [`FlatSpan`].
#[must_use]
pub fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
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

/// Start the mock on a free port and return its handle.
///
/// The HTTP server runs on its own thread + actix runtime; the test's
/// runtime is untouched.
#[must_use]
pub fn start() -> OtlpMock {
    // Bind to :0 to grab a free port, then drop the listener so the actix
    // runtime can re-bind to it. (Same trick as `et-ws-test-server`.)
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    start_on(port)
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
