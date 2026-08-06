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
        description = "ws-server HTTP surface: health probe, module discovery, module assets, and per-agent storage.
The storage routes are an anonymous S3-compatible interface -- addressed path-style as
/storage/{agent_id}/{filename} (bucket = agent_id, key = filename), they answer PUT/GET/HEAD with an ETag, so a
standard S3 client can read and write objects without credentials."
    ),
    servers(
        (url = "http://localhost:8080", description = "Default ws-server bind address")
    ),
    paths(
        et_ws_server::routes::health,
        et_modules_service::routes::list_modules_handler,
        et_modules_service::routes::get_module_file,
        et_storage_service::routes::get_file,
        et_storage_service::routes::head_file,
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
    clippy::unwrap_used,
    reason = "all conversions are between serde-derived types; the only way these expect calls fire is a serde_json bug"
)]
fn build_spec() -> openapiv3::OpenAPI {
    let mut doc = ApiDoc::openapi();
    doc.info.license = None;
    let mut value = serde_json::to_value(&doc).unwrap();
    if let Some(obj) = value.as_object_mut() {
        let _previous = obj.insert("openapi".into(), serde_json::Value::String("3.0.3".into()));
    }
    serde_json::from_value(value).unwrap()
}

/// Serialize the `OpenAPI` document as YAML for `generated/specs/rest.yaml`.
#[must_use]
#[expect(
    clippy::unwrap_used,
    reason = "openapiv3::OpenAPI is serde-derived and round-trips through serde_yaml unconditionally"
)]
pub fn render_yaml() -> String {
    serde_yaml::to_string(&build_spec()).unwrap()
}

/// Serialize the `OpenAPI` document as JSON.
///
/// Build intermediate consumed by `openapi2zig` (which doesn't accept YAML in
/// v0.2.0).
#[must_use]
#[expect(
    clippy::unwrap_used,
    reason = "openapiv3::OpenAPI is serde-derived and round-trips through serde_json unconditionally"
)]
pub fn render_json() -> String {
    serde_json::to_string_pretty(&build_spec()).unwrap()
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
    clippy::unwrap_used,
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
    let mut generator = progenitor::Generator::new(settings.with_pre_hook_async(trace_hook));
    let tokens = generator.generate_tokens(&spec)?;
    let ast = syn::parse2(tokens).unwrap();
    let body = prettyplease::unparse(&ast);
    let body = inject_wasm_baseurl_fallback(&body);
    let body = inject_retry_exec(&body);
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

/// Wrap progenitor's `pub fn new(baseurl: &str) -> Self` so that on
/// `wasm32-unknown-unknown` an empty `baseurl` falls back to the browser's
/// `window.location.origin` (in the embedded Deno runner, the bootstrap
/// stubs `globalThis.location` to the ws-server's HTTP base).
///
/// `reqwest`'s wasm32 build still parses URLs via `url::Url::parse`, which
/// rejects relative URLs with `RelativeUrlWithoutBase` -- so a browser
/// module that does `Client::new("")` expecting page-origin resolution
/// would otherwise fail every request with "Communication Error: builder
/// error" before any fetch leaves the wasm.
#[expect(
    clippy::single_call_fn,
    reason = "post-process step kept separate for readability; the named function documents intent"
)]
fn inject_wasm_baseurl_fallback(body: &str) -> String {
    // Anchor on progenitor's exact emitted prologue for `Client::new` so a
    // mismatch (e.g. upstream changing the cfg ordering) fails the build
    // here rather than silently producing a no-op replacement.
    let needle = r#"    pub fn new(baseurl: &str) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
"#;
    let replacement = r#"    pub fn new(baseurl: &str) -> Self {
        #[cfg(target_arch = "wasm32")]
        let baseurl_owned = if baseurl.is_empty() {
            ::web_sys::window()
                .and_then(|w| w.location().origin().ok())
                .unwrap_or_default()
        } else {
            baseurl.to_string()
        };
        #[cfg(target_arch = "wasm32")]
        let baseurl = baseurl_owned.as_str();
        #[cfg(not(target_arch = "wasm32"))]
"#;
    assert!(
        body.contains(needle),
        "progenitor's Client::new prologue moved; update inject_wasm_baseurl_fallback's needle"
    );
    body.replacen(needle, replacement, 1)
}

/// Give the generated client's `ClientHooks::exec` an exponential-backoff
/// retry loop (native targets only).
///
/// progenitor's per-call flow is `pre()` -> `exec()` -> `post()`, and `exec`'s
/// default just calls `self.client().execute(request)` once. We override it so
/// every REST call (module discovery, asset fetch, per-agent storage) tolerates
/// a transient failure -- in particular a ws-server that isn't up yet at
/// startup -- by retrying with backoff.
///
/// NOTE: we hand-inject this only because reqwest's own retry support
/// (`ClientBuilder::retries`, shipped in 0.12.23) has **no backoff yet** -- its
/// `tower::retry::Policy::retry` returns `std::future::Ready<()>` (retries fire
/// immediately) and the upstream `backoff` field is commented out behind a
/// `// TODO? backoff futures...`. When reqwest ships backoff, delete this hook
/// and configure `.retries()` on the `reqwest::Client` instead (see
/// <https://github.com/seanmonstar/reqwest/pull/2763>). The hook is
/// `#[cfg(not(wasm32))]` because `tokio::time::sleep` + `SystemTime` don't work
/// under `wasm32-unknown-unknown`; WASM consumers keep the default `exec`.
#[expect(
    clippy::single_call_fn,
    reason = "post-process step kept separate for readability; the named function documents intent"
)]
fn inject_retry_exec(body: &str) -> String {
    // progenitor emits an empty `impl ClientHooks` that takes the trait
    // defaults; we swap it for one carrying a custom `exec`.
    let needle = "impl ClientHooks<()> for &Client {}";
    let replacement = r#"impl ClientHooks<()> for &Client {
    // Injected by `utilities/int-gen` (inject_retry_exec): retry request
    // execution with exponential backoff. reqwest's native retry has no
    // backoff yet -- remove this and use `ClientBuilder::retries` once it does.
    #[cfg(not(target_arch = "wasm32"))]
    async fn exec(
        &self,
        request: ::reqwest::Request,
        _info: &OperationInfo,
    ) -> ::reqwest::Result<::reqwest::Response> {
        use ::retry_policies::policies::ExponentialBackoff;
        use ::retry_policies::{RetryDecision, RetryPolicy as _};
        let policy = ExponentialBackoff::builder()
            .retry_bounds(
                ::core::time::Duration::from_millis(250),
                ::core::time::Duration::from_secs(5),
            )
            .build_with_total_retry_duration(::core::time::Duration::from_secs(30));
        let started = ::std::time::SystemTime::now();
        let mut n_past_retries: u32 = 0;
        loop {
            // Retry only when the request can be replayed (no streaming body).
            let Some(attempt) = request.try_clone() else {
                return self.client().execute(request).await;
            };
            match self.client().execute(attempt).await {
                Ok(response) => return Ok(response),
                Err(err) => match policy.should_retry(started, n_past_retries) {
                    RetryDecision::Retry { execute_after } => {
                        let wait = execute_after
                            .duration_since(::std::time::SystemTime::now())
                            .unwrap_or_default();
                        ::tokio::time::sleep(wait).await;
                        n_past_retries = n_past_retries.saturating_add(1);
                    }
                    RetryDecision::DoNotRetry => return Err(err),
                },
            }
        }
    }
}"#;
    assert!(
        body.contains(needle),
        "progenitor's empty `impl ClientHooks` moved; update inject_retry_exec's needle"
    );
    body.replacen(needle, replacement, 1)
}
