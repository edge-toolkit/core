//! Emit `generated/specs/rest.yaml` and the typed Rust REST client at
//! `generated/rust-rest/src/lib.rs`.
//!
//! Paths and schemas are collected from the actual handlers in `et-ws-server`,
//! `et-storage-service`, and `et-modules-service` via `utoipa`; the client is
//! generated from the resulting `OpenAPI` document via `progenitor::Generator`.
//! Driving both steps from one Rust call keeps the spec and the client
//! guaranteed in sync -- no external CLI hop.

use utoipa::OpenApi;

use crate::Error;

// Each handler sets an explicit `tag = "..."` in its `#[utoipa::path]`
// attribute (see `et_*_service::routes`). Without that, utoipa derives
// the tag from the Rust path tokens -- turning
// `et_storage_service::routes::put_file` into the ugly
// `et_storage_serviceroutes`, which `openapi-python-client` then uses
// as the submodule name under `generated/python-rest/.../api/`.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "Edge Toolkit REST API",
        version = "0.1.0",
        description = "ws-server HTTP surface: health probe, module discovery, module assets, per-agent storage."
    ),
    servers(
        (url = "http://localhost:8080", description = "Default ws-server bind address")
    ),
    paths(
        et_ws_server::routes::health,
        et_modules_service::routes::list_modules_handler,
        et_modules_service::routes::get_module_file,
        et_storage_service::routes::get_file,
        et_storage_service::routes::put_file::<et_ws_server::AgentSession>,
    ),
    components(schemas(et_ws_server::routes::HealthResponse))
)]
struct ApiDoc;

/// Build the `openapiv3::OpenAPI` value once.
///
/// Both `rest.yaml` and the progenitor-generated client are derived from this.
/// `utoipa` unconditionally emits `openapi: 3.1.0` and `license.identifier`,
/// but progenitor 0.14 only accepts 3.0.x and rejects the `identifier` field --
/// downgrade those before serializing.
#[expect(
    clippy::expect_used,
    reason = "all conversions are between serde-derived types; the only way these expect calls fire is a serde_json bug"
)]
fn build_spec() -> openapiv3::OpenAPI {
    let mut doc = ApiDoc::openapi();
    doc.info.license = None;
    let mut value = serde_json::to_value(&doc).expect("OpenApi is always JSON-serializable");
    if let Some(obj) = value.as_object_mut() {
        let _previous = obj.insert("openapi".into(), serde_json::Value::String("3.0.3".into()));
    }
    serde_json::from_value(value).expect("downgraded OpenApi is always openapiv3::OpenAPI-shaped")
}

/// Serialize the `OpenAPI` document as YAML for `generated/specs/rest.yaml`.
#[must_use]
#[expect(
    clippy::expect_used,
    reason = "openapiv3::OpenAPI is serde-derived and round-trips through serde_yaml unconditionally"
)]
pub fn render_yaml() -> String {
    serde_yaml::to_string(&build_spec()).expect("openapiv3::OpenAPI is always YAML-serializable")
}

/// Serialize the `OpenAPI` document as JSON.
///
/// Build intermediate consumed by `openapi2zig` (which doesn't accept YAML in
/// v0.2.0).
#[must_use]
#[expect(
    clippy::expect_used,
    reason = "openapiv3::OpenAPI is serde-derived and round-trips through serde_json unconditionally"
)]
pub fn render_json() -> String {
    serde_json::to_string_pretty(&build_spec()).expect("openapiv3::OpenAPI is always JSON-serializable")
}

/// Generate the Rust REST client (`generated/rust-rest/src/lib.rs`) from the
/// same `OpenAPI` document via `progenitor::Generator`.
///
/// Same engine the retired `cargo-progenitor` CLI used, just driven in-process
/// so the only install target is the workspace itself.
///
/// The async pre-hook injects the W3C `traceparent` for the current tracing
/// span into every outgoing request, so the runner's span chain extends into
/// the server's `tracing-actix-web` request span end-to-end -- distributed
/// tracing works without each call site repeating the boilerplate the old
/// `inject_traceparent` helper did.
#[expect(
    clippy::expect_used,
    clippy::unwrap_in_result,
    reason = "progenitor's emit feeds straight into syn::parse2; a parse failure means progenitor produced invalid Rust"
)]
pub fn render_rust_client() -> Result<String, Error> {
    let spec = build_spec();

    // progenitor splices `(#hook)(&mut request).await` into every generated
    // method. We hand it a closure that mutates the request synchronously
    // (cheap, no I/O) and then returns a trivially-Ok async block,
    // sidestepping the still-unstable `async ||` closures. The OTel
    // injection itself is `#[cfg(feature = "tracing")]`-gated so WASM
    // consumers (e.g. the browser data1 module) can disable the feature
    // and avoid pulling in the opentelemetry/tracing-opentelemetry deps,
    // which don't compile on `wasm32-unknown-unknown`.
    let trace_hook = quote::quote! {
        |request: &mut ::reqwest::Request| {
            #[cfg(feature = "tracing")]
            {
                let cx = <::tracing::Span as ::tracing_opentelemetry::OpenTelemetrySpanExt>::context(
                    &::tracing::Span::current(),
                );
                ::opentelemetry::global::get_text_map_propagator(|propagator| {
                    propagator.inject_context(
                        &cx,
                        &mut ::opentelemetry_http::HeaderInjector(request.headers_mut()),
                    );
                });
            }
            #[cfg(not(feature = "tracing"))]
            let _ = request;
            async { Ok::<(), ::std::convert::Infallible>(()) }
        }
    };

    let mut settings = progenitor::GenerationSettings::default();
    let _settings = settings.with_pre_hook_async(trace_hook);
    let mut generator = progenitor::Generator::new(&settings);
    let tokens = generator.generate_tokens(&spec)?;
    let ast = syn::parse2(tokens).expect("progenitor always emits valid Rust");
    let body = prettyplease::unparse(&ast);
    // progenitor wraps each handler doc as a `/**...*/` block, immediately
    // appending its own `Sends a METHOD request to /path` line at +4 spaces;
    // any non-empty description from the `OpenAPI` spec then leaves that
    // following block indented by 4 from the first line, which CommonMark
    // promotes to an indented code block -- rustdoc then tries to compile it
    // as Rust and trips on the surrounding backticks. Allow the rustdoc
    // codeblock lint at the generated-file level rather than fighting the
    // upstream emit (or rewriting every public-API description to dodge the
    // markdown rule).
    Ok(format!("#![allow(rustdoc::invalid_rust_codeblocks)]\n{body}"))
}
