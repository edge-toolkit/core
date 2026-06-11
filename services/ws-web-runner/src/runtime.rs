//! Deno `MainWorker` setup and module evaluation.
//!
//! `MainWorker` (from `deno_runtime`) handles the heavy lifting we used
//! to do by hand: timers, `fetch`, `WebSocket`, `Headers`/`Request`/
//! `Response`, crypto, `localStorage`, `Event`/`EventTarget`, `URL`,
//! `Blob`, `File`, base64, performance, and console -- all wired onto
//! `globalThis` by its bootstrap. We layer on top:
//!   - a custom module loader that fetches from the ws-server
//!   - browser-environment shims for wasm-bindgen modules (`window` /
//!     `location` / `navigator` / `document` / `HTMLCanvasElement`)
//!   - `WebAssembly.{instantiate,compile}Streaming` patches that fall
//!     back to `arrayBuffer + instantiate/compile`, because `deno_fetch`
//!     `Response` objects aren't streamable in V8's native path (dotnet
//!     hits this)
//!   - a `globalThis`-level event-target shim Pyodide expects
//!   - removing the `Deno` global (Pyodide sniffs it and takes a
//!     broken path)

use std::rc::Rc;
use std::sync::Arc;

use deno_core::error::CoreError;
use deno_core::futures::FutureExt as _;
use deno_core::v8::{BackingStore, SharedRef};
use deno_core::{CrossIsolateStore, ModuleLoadResponse, ModuleSpecifier};
use deno_resolver::npm::{DenoInNpmPackageChecker, NpmResolver};
use deno_runtime::deno_fetch::dns::Resolver;
use deno_runtime::deno_fs::{FileSystemRc, RealFs};
use deno_runtime::deno_inspector_server::MainInspectorSessionChannel;
use deno_runtime::deno_io::Stdio;
use deno_runtime::deno_permissions::PermissionsContainer;
use deno_runtime::deno_web::InMemoryBroadcastChannel;
use deno_runtime::ops::worker_host::CreateWebWorkerCb;
use deno_runtime::permissions::RuntimePermissionDescriptorParser;
use deno_runtime::web_worker::{WebWorker, WebWorkerOptions, WebWorkerServiceOptions};
use deno_runtime::worker::{MainWorker, WorkerOptions, WorkerServiceOptions};
use deno_runtime::{BootstrapOptions, WorkerExecutionMode};
use et_rest_client::ClientInfo as _;
use sys_traits::impls::RealSys;

use crate::error::JsErrExt as _;

/// Module loader that fetches JavaScript from the ws-server over HTTP.
///
/// Uses the typed REST client's inner `reqwest::Client` so the loader can
/// fetch arbitrary module sub-paths (ES `import("/modules/x/sub/dir/y.js")`)
/// that the typed `get_module_file` would percent-encode incorrectly via
/// `encode_path` (slashes inside the path segment get turned into `%2F`).
struct ServerModuleLoader {
    rest: et_rest_client::Client,
}

impl deno_core::ModuleLoader for ServerModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: deno_core::ResolutionKind,
    ) -> deno_core::ModuleResolveResponse {
        // Absolute URLs pass through directly.
        if let Ok(url) = ModuleSpecifier::parse(specifier) {
            return Ok(url);
        }

        // Server-root-relative paths like "/modules/et-ws-wasm-agent/..."
        if specifier.starts_with('/') {
            let url = format!("{}{specifier}", self.rest.baseurl());
            return ModuleSpecifier::parse(&url).map_js_err();
        }

        // Relative paths resolved against the referrer.
        let base = ModuleSpecifier::parse(referrer)
            .map_js_err_with_context(|| format!("bad referrer (referrer={referrer:?}, specifier={specifier:?})"))?;
        base.join(specifier).map_js_err()
    }

    fn load(
        &self,
        module_specifier: &ModuleSpecifier,
        _maybe_referrer: Option<&deno_core::ModuleLoadReferrer>,
        _options: deno_core::ModuleLoadOptions,
    ) -> ModuleLoadResponse {
        let url = module_specifier.clone();
        let client = self.rest.client().clone();
        ModuleLoadResponse::Async(
            async move {
                // Retry transport-level send failures. reqwest reuses pooled
                // keep-alive connections; the server can close an idle one in
                // the gap since the previous request (MainWorker bootstrap can
                // outlast the server's keep-alive), so the first send() fails
                // with "error sending request". A fresh attempt dials anew.
                let mut attempt = 0u8;
                let response = loop {
                    match client.get(url.as_str()).send().await {
                        Ok(response) => break response,
                        Err(e) if (e.is_request() || e.is_connect()) && attempt < 2 => {
                            attempt = attempt.saturating_add(1);
                            tracing::warn!(url = %url, attempt, error = %e, "module fetch send error; retrying");
                        }
                        Err(e) => return Err(e).map_js_err(),
                    }
                };

                let body = response.error_for_status().map_js_err()?.text().await.map_js_err()?;
                let specifier = ModuleSpecifier::parse(url.as_str()).map_js_err()?;

                Ok(deno_core::ModuleSource::new(
                    deno_core::ModuleType::JavaScript,
                    deno_core::ModuleSourceCode::String(body.into()),
                    &specifier,
                    None,
                ))
            }
            .boxed_local(),
        )
    }
}

/// The browser-environment shim layered on top of `MainWorker`'s bootstrap.
///
/// Sourced from `shim.js` next to this file, with two placeholder tokens
/// substituted for the per-run URLs.
const SHIM_TEMPLATE: &str = include_str!("shim.js");

/// Render the shim with the per-run URL substitutions.
///
/// Substitutes the ws-server's HTTP base (used for `location` and module URL
/// resolution) and the WebSocket URL (exposed on `globalThis.__ET_WS_URL`).
fn shim_js(http_base: &str, ws_url: &str) -> String {
    SHIM_TEMPLATE
        .replace("__ET_HTTP_BASE__", http_base)
        .replace("__ET_WS_URL__", ws_url)
}

/// Build the `CreateWebWorkerCb` that spawns child `WebWorker`s on fresh OS threads.
///
/// The closure captures the bits a worker needs (REST client to build its own
/// module loader, the `fs`, the cross-isolate `SharedArrayBuffer` store) and
/// recurses by handing itself (cloned `Arc`) to each child so workers can spawn
/// grand-children.
fn create_web_worker_cb(
    rest: et_rest_client::Client,
    fs: FileSystemRc,
    sab_store: CrossIsolateStore<SharedRef<BackingStore>>,
    http_base: String,
    ws_url: String,
) -> Arc<CreateWebWorkerCb> {
    Arc::new(move |args| {
        let rest = rest.clone();
        let fs = Arc::clone(&fs);
        let sab_store = sab_store.clone();
        let http_base = http_base.clone();
        let ws_url = ws_url.clone();

        let module_loader: Rc<dyn deno_core::ModuleLoader> = Rc::new(ServerModuleLoader { rest: rest.clone() });

        // Recurse so this worker can spawn its own children.
        let create_web_worker_cb = create_web_worker_cb(
            rest,
            Arc::clone(&fs),
            sab_store.clone(),
            http_base.clone(),
            ws_url.clone(),
        );

        let services = WebWorkerServiceOptions::<DenoInNpmPackageChecker, NpmResolver<RealSys>, RealSys> {
            fs,
            module_loader,
            permissions: args.permissions,
            shared_array_buffer_store: Some(sab_store),
            blob_store: Arc::default(),
            broadcast_channel: InMemoryBroadcastChannel::default(),
            bundle_provider: None,
            compiled_wasm_module_store: None,
            deno_rt_native_addon_loader: None,
            feature_checker: Arc::default(),
            main_inspector_session_tx: MainInspectorSessionChannel::default(),
            node_services: None,
            npm_process_state_provider: None,
            root_cert_store_provider: None,
        };

        let options = WebWorkerOptions {
            bootstrap: BootstrapOptions {
                location: Some(args.main_module.clone()),
                mode: WorkerExecutionMode::Worker,
                user_agent: concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION")).to_string(),
                close_on_idle: false,
                ..Default::default()
            },
            create_web_worker_cb,
            main_module: args.main_module.clone(),
            name: args.name,
            worker_id: args.worker_id,
            worker_type: args.worker_type,
            cache_storage_dir: None,
            close_on_idle: false,
            create_params: None,
            enable_raw_imports: false,
            enable_stack_trace_arg_in_ops: false,
            extensions: vec![],
            format_js_error_fn: None,
            maybe_coverage_dir: None,
            maybe_cpu_prof_config: None,
            maybe_worker_metadata: None,
            residual_lazy_esm_sources: &[],
            residual_lazy_js_sources: &[],
            seed: None,
            startup_snapshot: None,
            stdio: Stdio::default(),
            trace_ops: None,
            unsafely_ignore_certificate_errors: None,
        };

        // Pre-shim the worker so browser-environment fakes are in place
        // before any module code runs. `bootstrap_from_options` returns
        // a `(WebWorker, SendableWebWorkerHandle)` tuple.
        let (mut worker, handle) = WebWorker::bootstrap_from_options(services, options);
        let shim = shim_js(&http_base, &ws_url);
        // No `Result` channel in this callback, so log and continue.
        if let Err(e) = worker.js_runtime.execute_script("<web-runner-worker-shim>", shim) {
            tracing::error!(
                error = ?e,
                "worker environment shim failed unexpectedly (it already succeeded on the main thread)"
            );
        }
        (worker, handle)
    })
}

/// Construct a `MainWorker` and run the entry-point module.
///
/// `MainWorker` brings the standard web-platform globals (fetch,
/// WebSocket, timers, etc.); `shim_js` layers the browser-environment
/// fakes browser-targeted WASM expects on top. `create_web_worker_cb`
/// hooks `new Worker(...)` to spawn fresh `WebWorker`s on their own
/// threads sharing a `BackingStore` store for `SharedArrayBuffer`
/// cross-isolate transfer.
#[expect(
    clippy::single_call_fn,
    clippy::future_not_send,
    reason = "MainWorker is !Send; called from single-threaded tokio"
)]
pub async fn run_js_module(
    entry_url: &str,
    http_base: &str,
    ws_url: &str,
    rest: et_rest_client::Client,
) -> Result<(), CoreError> {
    let module_loader: Rc<dyn deno_core::ModuleLoader> = Rc::new(ServerModuleLoader { rest: rest.clone() });

    let sys = RealSys;
    let permission_desc_parser = Arc::new(RuntimePermissionDescriptorParser::new(sys));
    let permissions = PermissionsContainer::allow_all(permission_desc_parser);

    let fs: FileSystemRc = Arc::new(RealFs);
    let sab_store: CrossIsolateStore<SharedRef<BackingStore>> = CrossIsolateStore::default();

    let service_options = WorkerServiceOptions::<DenoInNpmPackageChecker, NpmResolver<RealSys>, RealSys> {
        fs: Arc::clone(&fs),
        module_loader,
        permissions,
        shared_array_buffer_store: Some(sab_store.clone()),
        blob_store: Arc::default(),
        broadcast_channel: InMemoryBroadcastChannel::default(),
        bundle_provider: None,
        compiled_wasm_module_store: None,
        deno_rt_native_addon_loader: None,
        feature_checker: Arc::default(),
        fetch_dns_resolver: Resolver::default(),
        node_services: None,
        npm_process_state_provider: None,
        root_cert_store_provider: None,
        v8_code_cache: None,
    };

    let bootstrap = BootstrapOptions {
        close_on_idle: true,
        location: Some(ModuleSpecifier::parse(http_base).map_js_err()?),
        mode: WorkerExecutionMode::Run,
        user_agent: concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION")).to_string(),
        ..Default::default()
    };

    let create_web_worker_cb = create_web_worker_cb(rest, fs, sab_store, http_base.to_string(), ws_url.to_string());
    let main_specifier = ModuleSpecifier::parse(entry_url).map_js_err()?;
    let mut worker = MainWorker::bootstrap_from_options::<DenoInNpmPackageChecker, NpmResolver<RealSys>, RealSys>(
        &main_specifier,
        service_options,
        WorkerOptions {
            bootstrap,
            create_web_worker_cb,
            extensions: vec![],
            startup_snapshot: None,
            ..Default::default()
        },
    );

    // Apply our browser-environment shims on top of Deno's globals. The
    // `Global<v8::Value>` returned by `execute_script` is just the
    // last-expression result; we don't need it.
    drop(
        worker
            .js_runtime
            .execute_script("<web-runner-shim>", shim_js(http_base, ws_url))?,
    );

    // Load + run the module via an inline wrapper: dynamic `import()` works
    // from ES-module context but not from `execute_script`, so synthesise a
    // side ES module.
    let wrapper_code = format!(
        r#"
const mod = await import("{entry_url}");
let invoked = false;
if (typeof mod.default === "function") {{
    await mod.default();
    invoked = true;
}}
if (typeof mod.run === "function") {{
    await mod.run();
    invoked = true;
}}
if (!invoked) {{
    throw new Error("module {entry_url} exports neither a `default` nor a `run` function");
}}
"#
    );
    let wrapper_specifier = ModuleSpecifier::parse("internal:///runner-wrapper.js")?;
    let wrapper_id = worker
        .js_runtime
        .load_side_es_module_from_code(&wrapper_specifier, wrapper_code)
        .await?;
    let eval_future = std::pin::pin!(worker.js_runtime.mod_evaluate(wrapper_id));
    worker
        .js_runtime
        .with_event_loop_promise(eval_future, deno_core::PollEventLoopOptions::default())
        .await?;

    drop(worker);
    Ok(())
}
