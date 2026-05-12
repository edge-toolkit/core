//! In-process mock OTLP/HTTP-JSON collector.
//!
//! Used by integration tests to verify trace-context propagation across
//! processes: both the system-under-test (ws-server, ws-wasi-runner, ...)
//! point their OTLP exporters at this collector, then the test reads the
//! captured spans back to assert that trace ids match.
//!
//! This is **not** a real OTLP implementation — it just buffers JSON
//! payloads. The endpoints match the URL shape `et-otlp::init` produces:
//!
//!   - `POST <collector_url>/traces`
//!   - `POST <collector_url>/logs`
//!
//! so tests should set `OTLP_COLLECTOR_URL=<mock.collector_url>` and
//! `OTLP_PROTOCOL=JSON`.

use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use actix_web::{App, HttpResponse, HttpServer, post, web};
use serde_json::Value;

#[derive(Default)]
struct Captured {
    traces: Mutex<Vec<Value>>,
    logs: Mutex<Vec<Value>>,
}

/// Handle to a running mock collector. The server is shut down when this
/// struct is dropped (the actix runtime is owned by the spawned thread —
/// when our struct goes out of scope, the spawned thread's tokio runtime
/// stays alive but the handle pointing at it is dropped, which is fine for
/// test scope).
pub struct OtlpMock {
    /// Pass this to `OTLP_COLLECTOR_URL` in env so OTLP exporters target
    /// the mock. Trace endpoint is `<collector_url>/traces`; logs is
    /// `<collector_url>/logs` — matches `et_otlp::init`'s URL convention.
    pub collector_url: String,
    captured: Arc<Captured>,
}

impl OtlpMock {
    /// Snapshot the trace payloads received so far. Each element is one
    /// `ExportTraceServiceRequest` body (top-level shape:
    /// `{ "resourceSpans": [...] }`).
    #[must_use]
    pub fn traces(&self) -> Vec<Value> {
        self.captured.traces.lock().unwrap().clone()
    }

    /// Snapshot the log payloads received so far.
    #[must_use]
    pub fn logs(&self) -> Vec<Value> {
        self.captured.logs.lock().unwrap().clone()
    }

    /// Walk every span across every captured request, returning each span
    /// with its parent `Resource`'s `service.name` attribute (so the test
    /// can group spans by service). Trace and span ids stay as the
    /// base64-encoded strings the OTLP/HTTP-JSON encoding uses — equality
    /// comparison is what tests need, not decoding.
    #[must_use]
    pub fn flatten_spans(&self) -> Vec<FlatSpan> {
        let mut out = Vec::new();
        for req in self.traces() {
            let Some(resource_spans) = req.get("resourceSpans").and_then(Value::as_array) else {
                continue;
            };
            for rs in resource_spans {
                let service_name = rs
                    .get("resource")
                    .and_then(|r| r.get("attributes"))
                    .and_then(Value::as_array)
                    .and_then(|attrs| {
                        attrs.iter().find_map(|attr| {
                            if attr.get("key").and_then(Value::as_str) == Some("service.name") {
                                attr.get("value")
                                    .and_then(|v| v.get("stringValue"))
                                    .and_then(Value::as_str)
                                    .map(str::to_string)
                            } else {
                                None
                            }
                        })
                    })
                    .unwrap_or_default();
                let Some(scope_spans) = rs.get("scopeSpans").and_then(Value::as_array) else {
                    continue;
                };
                for ss in scope_spans {
                    let Some(spans) = ss.get("spans").and_then(Value::as_array) else {
                        continue;
                    };
                    for span in spans {
                        let trace_id = span.get("traceId").and_then(Value::as_str).unwrap_or("").to_string();
                        let span_id = span.get("spanId").and_then(Value::as_str).unwrap_or("").to_string();
                        let parent_span_id = span
                            .get("parentSpanId")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let name = span.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                        out.push(FlatSpan {
                            service_name: service_name.clone(),
                            trace_id,
                            span_id,
                            parent_span_id,
                            name,
                        });
                    }
                }
            }
        }
        out
    }
}

/// Flattened span view for assertions.
#[derive(Clone, Debug)]
pub struct FlatSpan {
    pub service_name: String,
    /// Base64-encoded 16-byte trace id (OTLP/HTTP-JSON proto-JSON encoding).
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: String,
    pub name: String,
}

#[post("/traces")]
async fn handle_traces(state: web::Data<Arc<Captured>>, body: web::Json<Value>) -> HttpResponse {
    state.traces.lock().unwrap().push(body.into_inner());
    // OTLP success: empty `ExportTraceServiceResponse` is just `{}`.
    HttpResponse::Ok().content_type("application/json").body("{}")
}

#[post("/logs")]
async fn handle_logs(state: web::Data<Arc<Captured>>, body: web::Json<Value>) -> HttpResponse {
    state.logs.lock().unwrap().push(body.into_inner());
    HttpResponse::Ok().content_type("application/json").body("{}")
}

/// Start the mock on a free port and return its handle. The HTTP server
/// runs on its own thread + actix runtime; the test's runtime is untouched.
pub fn start() -> OtlpMock {
    // Bind to :0 to grab a free port, then drop the listener so the actix
    // runtime can re-bind to it. (Same trick as `et-ws-test-server`.)
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let captured = Arc::new(Captured::default());
    let captured_for_server = captured.clone();
    let addr = format!("127.0.0.1:{port}");

    std::thread::spawn(move || {
        actix_rt::System::new().block_on(async move {
            let data = web::Data::new(captured_for_server);
            HttpServer::new(move || {
                App::new()
                    .app_data(data.clone())
                    .app_data(web::JsonConfig::default().limit(64 * 1024 * 1024))
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
    for _ in 0..50 {
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
