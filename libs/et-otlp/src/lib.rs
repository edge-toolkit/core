//! Shared `OpenTelemetry` / OTLP setup for edge-toolkit services.
//!
//! Wires up:
//! - The W3C tracecontext propagator (so `traceparent` headers cross
//!   process boundaries on HTTP).
//! - An OTLP/HTTP span exporter (binary or JSON, per `OtlpConfig`).
//! - An OTLP/HTTP log exporter, exposed through `tracing` so `info!` and
//!   friends are forwarded.
//! - A `tracing` subscriber that fans `info!`/`error!` out to stdout *and*
//!   the `OTel` pipeline.
//!
//! Returns an `OtelHandles` which the caller must `shutdown()` before exit
//! so batched spans/logs are flushed -- otherwise short-lived processes
//! (e.g. the wasi-runner, which exits as soon as a module finishes) drop
//! their tail-end spans.
use edge_toolkit::config::{OtlpConfig, OtlpProtocol};
use opentelemetry::{KeyValue, trace::TracerProvider as _};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{LogExporter, MetricExporter, WithExportConfig as _, WithHttpConfig as _};
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::{Resource, propagation::TraceContextPropagator};
use tracing::subscriber::set_global_default;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::{EnvFilter, Registry, layer::SubscriberExt as _};

pub const RUST_LOG: &str = "RUST_LOG";

/// Handles for the spans + logs + metrics pipelines.
///
/// Drop alone won't flush -- call [`OtelHandles::shutdown`] at the end of
/// `main()` (or in a Drop guard).
#[non_exhaustive]
pub struct OtelHandles {
    pub tracer_provider: SdkTracerProvider,
    pub logger_provider: SdkLoggerProvider,
    pub meter_provider: SdkMeterProvider,
}

impl OtelHandles {
    /// Flush any buffered spans/logs/metrics and tear down the exporters.
    pub fn shutdown(self) {
        // Errors here are non-fatal -- the process is exiting anyway, and there is no caller to
        // propagate to, so each teardown result is intentionally discarded.
        let _tracer = self.tracer_provider.shutdown();
        let _logger = self.logger_provider.shutdown();
        let _meter = self.meter_provider.shutdown();
    }
}

/// The HTTP headers every OTLP exporter is built with, carrying basic auth when the config supplies it.
///
/// Split out of [`init`] so both shapes are reachable from a test. `init` installs a process-global
/// subscriber and can therefore run at most once per process, which leaves anything decided inside it
/// testable only in whichever configuration that single call happens to use -- and a collector reached
/// without its credentials rejects every export, silently, for the life of the process.
#[must_use]
pub fn exporter_headers(auth: Option<&edge_toolkit::auth::BasicAuth>) -> std::collections::HashMap<String, String> {
    let mut headers = std::collections::HashMap::new();
    if let Some(auth) = auth {
        auth.add_basic_auth_header(&mut headers);
    }
    headers
}

/// The resource attributes describing this service, given whatever the host's name resolved to.
///
/// Takes the hostname rather than reading it, for the same reason as [`exporter_headers`] and one more:
/// the `None` case is a host whose name does not resolve or is not UTF-8, which no test can bring about
/// by asking the real machine. Passing it in is what makes the attribute's absence an asserted behaviour
/// instead of an assumption -- the resource must still carry the version, and simply omit the instance.
#[must_use]
pub fn service_descriptors(hostname: Option<String>) -> Vec<KeyValue> {
    let mut descriptors = vec![KeyValue::new("service.version", env!("CARGO_PKG_VERSION").to_string())];
    if let Some(hostname) = hostname {
        descriptors.push(KeyValue::new("service.instance", hostname));
    }
    descriptors
}

/// Telemetry for the life of a scope, flushed when it ends however it ends.
///
/// [`OtelHandles`] has to be shut down explicitly, which every binary holding one has to remember to do on
/// every exit path -- and the paths that matter most are the failing ones, whose spans are exactly what the
/// batch exporter is still holding. Tying the flush to a drop makes forgetting impossible and leaves `main`
/// free to use `?` again.
pub struct TelemetryGuard(Option<OtelHandles>);

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Some(handles) = self.0.take() {
            handles.shutdown();
        }
    }
}

/// Initialise telemetry as [`init_or_stderr`] does, flushing it when the returned guard drops.
///
/// # Errors
///
/// Returns whatever [`init`] does; the fallback path cannot fail.
pub fn init_guarded(config: Option<&OtlpConfig>) -> Result<TelemetryGuard, Box<dyn std::error::Error + Send + Sync>> {
    init_or_stderr(config).map(TelemetryGuard)
}

/// A config that says where its process exports telemetry.
///
/// Exists so [`load_telemetered`] can read a config and start its pipeline in one step, whatever the rest of
/// that config holds.
pub trait Telemetered {
    /// Where this process exports telemetry, or `None` to log to stderr.
    fn otlp(&self) -> Option<&OtlpConfig>;
}

/// Read a config from the environment and start the telemetry it describes.
///
/// The two belong together: the pipeline is configured by what was just read, and its guard has to outlive
/// the run so the exporter's last batch is flushed. One call is what stops a binary loading a config and
/// then forgetting the half that makes its spans reach anything -- which is how two of this project's three
/// runners came to have no telemetry at all.
///
/// # Errors
///
/// Returns the deserialisation error if the environment does not describe a valid config, or whatever
/// starting the pipeline reports.
pub fn load_telemetered<C>() -> Result<(C, TelemetryGuard), Box<dyn std::error::Error + Send + Sync>>
where
    C: serde::de::DeserializeOwned + Telemetered,
{
    let config = serde_env::from_env::<C>()?;
    let telemetry = init_guarded(config.otlp())?;
    Ok((config, telemetry))
}

/// Initialise telemetry from `config`, falling back to plain stderr logging when there is none.
///
/// Every runner faces the same choice -- a deployment that configured `OTLP_*` wants the pipeline, one that
/// did not still wants its logs somewhere -- and having each binary spell it out is how they drift: two of
/// the three runners silently had no telemetry at all until this existed, and nothing said so.
///
/// Returning the handles rather than installing an exit hook keeps the flush the caller's to place. It has to
/// happen before the process ends and after the work does, and only the caller knows where that is.
///
/// # Errors
///
/// Returns whatever [`init`] does; the fallback path cannot fail.
pub fn init_or_stderr(
    config: Option<&OtlpConfig>,
) -> Result<Option<OtelHandles>, Box<dyn std::error::Error + Send + Sync>> {
    let Some(config) = config else {
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
            .init();
        return Ok(None);
    };
    init(config).map(Some)
}

/// Initialise the global tracing subscriber + `OTel` pipeline against `config`.
///
/// Call exactly once per process; a second call returns an error from
/// `set_global_default`. Exporter-build and `RUST_LOG`-parse failures are
/// returned too, so `main` can surface them and exit non-zero.
///
/// # Errors
///
/// Returns an error if any OTLP exporter fails to build, `RUST_LOG` is
/// invalid, or the global subscriber is already set.
pub fn init(config: &OtlpConfig) -> Result<OtelHandles, Box<dyn std::error::Error + Send + Sync>> {
    // tracing_log forwards `log` crate records (used by transitive deps)
    // through the tracing subscriber. A second init (global logger already set) is a real error here.
    tracing_log::LogTracer::init()?;

    let headers = exporter_headers(config.auth.as_ref());

    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

    let trace_endpoint = format!("{}/traces", config.collector_url);
    let log_endpoint = format!("{}/logs", config.collector_url);
    let protocol = match config.protocol {
        OtlpProtocol::Binary => opentelemetry_otlp::Protocol::HttpBinary,
        OtlpProtocol::JSON => opentelemetry_otlp::Protocol::HttpJson,
    };

    let span_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_protocol(protocol)
        .with_endpoint(trace_endpoint)
        .with_headers(headers.clone())
        .build()?;

    let service_descriptors = service_descriptors(hostname::get().ok().and_then(|host| host.into_string().ok()));
    let resource = Resource::builder()
        .with_service_name(config.service_label.clone())
        .with_attributes(service_descriptors)
        .build();

    let tracer_provider = SdkTracerProvider::builder()
        .with_batch_exporter(span_exporter)
        .with_resource(resource.clone())
        .build();
    // Set the global tracer provider so direct `global::tracer(...)` spans (e.g. the ws hub's `ws.connect`)
    // export too -- not just the `tracing`-subscriber spans routed through the layer below.
    opentelemetry::global::set_tracer_provider(tracer_provider.clone());

    let otel_tracing_layer = OpenTelemetryLayer::new(tracer_provider.tracer(config.service_label.clone()));

    // Metrics ride the same OTLP/HTTP transport as spans and logs, posting to `<collector_url>/metrics`.
    // The periodic reader batches on its own interval; `OtelHandles::shutdown` forces a final flush on exit.
    let metric_endpoint = format!("{}/metrics", config.collector_url);
    let metric_exporter = MetricExporter::builder()
        .with_http()
        .with_protocol(protocol)
        .with_endpoint(metric_endpoint)
        .with_headers(headers.clone())
        .build()?;
    let meter_provider = SdkMeterProvider::builder()
        .with_periodic_exporter(metric_exporter)
        .with_resource(resource.clone())
        .build();
    opentelemetry::global::set_meter_provider(meter_provider.clone());

    let log_directives = std::env::var(RUST_LOG).unwrap_or_else(|_| "info".to_string());
    let env_filter = EnvFilter::try_new(log_directives)?;

    let log_exporter = LogExporter::builder()
        .with_http()
        .with_protocol(protocol)
        .with_endpoint(log_endpoint)
        .with_headers(headers)
        .build()?;

    let logger_provider = SdkLoggerProvider::builder()
        .with_batch_exporter(log_exporter)
        .with_resource(resource)
        .build();

    let otel_log_layer = OpenTelemetryTracingBridge::new(&logger_provider);
    let stdout_fmt_layer = tracing_subscriber::fmt::layer().event_format(tracing_subscriber::fmt::format().compact());

    let subscriber = Registry::default()
        .with(env_filter)
        .with(stdout_fmt_layer)
        .with(otel_tracing_layer)
        .with(otel_log_layer);

    set_global_default(subscriber)?;

    Ok(OtelHandles {
        tracer_provider,
        logger_provider,
        meter_provider,
    })
}
