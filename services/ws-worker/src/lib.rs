use std::time::Duration;

use rustyscript::deno_core::error::ModuleLoaderError;
use rustyscript::deno_core::{ModuleSource, ModuleSourceCode, ModuleSpecifier};
use rustyscript::module_loader::ImportProvider;
use rustyscript::{ExtensionOptions, Runtime, RuntimeOptions, WebOptions};

pub fn derive_http_base(ws_url: &str) -> Option<String> {
    let (scheme, rest) = if let Some(r) = ws_url.strip_prefix("wss://") {
        ("https", r)
    } else if let Some(r) = ws_url.strip_prefix("ws://") {
        ("http", r)
    } else {
        return None;
    };
    let host_port = rest.strip_suffix("/ws").unwrap_or(rest);
    Some(format!("{scheme}://{host_port}"))
}

/// Prepends a `Request` shim to every JS module fetched from the server so that
/// relative URLs are resolved against the server base before reaching Deno's native
/// fetch. Also rewrites the wasm-bindgen `arg0.fetch(getStringFromWasm0(...))` call
/// site, since `globalThis.fetch` is a non-writable native binding that cannot be
/// intercepted via property assignment.
pub struct RelativeUrlFixup {
    pub http_base: String,
}

impl ImportProvider for RelativeUrlFixup {
    fn post_process(
        &mut self,
        specifier: &ModuleSpecifier,
        mut source: ModuleSource,
    ) -> Result<ModuleSource, ModuleLoaderError> {
        if specifier.scheme() != "http" && specifier.scheme() != "https" {
            return Ok(source);
        }
        let base = &self.http_base;
        let original = String::from_utf8_lossy(source.code.as_bytes()).into_owned();

        // `var` (not `const`/`let`) avoids TDZ; `globalThis.Request`/`globalThis.fetch`
        // reference the native binding even after these declarations shadow the
        // free-variable name. `globalThis.fetch` is non-writable AND non-configurable,
        // so we can't replace it globally — module-scope shadowing is the only way to
        // intercept `fetch('/path')` calls that originate from module code.
        let request_shim = format!(
            r#"
            var Request = function(input, init) {{
              if (typeof input === 'string' && input[0] === '/') {{
                  input = {base:?} + input;
              }}
              return new globalThis.Request(input, init);
            }};
            var fetch = function(input, init) {{
                if (typeof input === 'string' && input[0] === '/') {{
                    input = {base:?} + input;
                }}
                return globalThis.fetch(input, init);
            }};
            "#
        );

        let patched = original.replace(
            "const ret = arg0.fetch(getStringFromWasm0(arg1, arg2));",
            &format!(
                r#"
                let __u = getStringFromWasm0(arg1, arg2);
                if (__u[0] === '/') __u = {base:?} + __u;
                const ret = arg0.fetch(__u);
                "#
            ),
        );

        // Patch `loadPyodideScript()` in pyodide-based modules: replace the
        // `document.createElement("script")` approach with fetch+eval.
        // We use fetch+eval (not import()) because:
        //   1. import() bypasses ImportProvider.post_process, so we can't patch pyodide.js
        //   2. eval() runs in the global scope, making loadPyodide available as a global
        //   3. pyodide.js is a UMD script that sets globalThis.loadPyodide directly
        // Also pass `indexURL` to `loadPyodide()` so it fetches pyodide assets from the server.
        let patched = patched.replace(
            r#"function loadPyodideScript() {
  return new Promise((resolve, reject) => {
    if (globalThis.loadPyodide) return resolve();
    const s = document.createElement("script");
    s.src = PYODIDE_CDN;
    s.onload = resolve;
    s.onerror = reject;
    document.head.appendChild(s);
  });
}"#,
            r#"async function loadPyodideScript() {
  if (globalThis.loadPyodide) return;
  const url = new URL(PYODIDE_CDN, import.meta.url).href;
  const text = await fetch(url).then(r => r.text());
  // Hide Deno from pyodide so its env detection reports IN_DENO=false,
  // leaving IN_BROWSER=true as the active code path. The Deno code path
  // skips creating a MessageChannel that pyproxy bootstrap later requires.
  // Pyodide captures env at load-time, so Deno is restored after eval —
  // Deno.core.* must remain available for rustyscript's ops afterwards.
  // With Deno hidden and no `process`/`document`, none of pyodide's
  // loadScript branches match, so we still need the throw fallback.
  // The loadScript variable's minified name differs between pyodide.js (k) and
  // pyodide.asm.js (Fe). We capture it from the preceding `IN_SHELL` branch,
  // so the throw fallback assigns to the correct local variable instead of
  // clobbering an unrelated `k` (which in pyodide.asm.js is the PyContainsMethods class).
  const patch = (s) => s
    .replace(/J\.runInThisContext\(/g, '(0,eval)(')
    .replace(
      new RegExp(
        'else if\\(([a-zA-Z_$][\\w$]*)\\.IN_SHELL\\)([a-zA-Z_$][\\w$]*)=load;'
        + 'else throw new Error\\("Cannot determine runtime environment"\\)',
        'g'),
      'else if($1.IN_SHELL)$2=load;else $2=async function(u){return globalThis.__pyEval(u)}'
    );
  globalThis.__pyEval = async function(e) {
    const t = patch(await (await fetch(e)).text());
    const sd = globalThis.Deno;
    globalThis.Deno = undefined;
    try { (0, eval)(t); } finally { globalThis.Deno = sd; }
  };
  const patched = patch(text);
  const sd = globalThis.Deno;
  globalThis.Deno = undefined;
  try { (0, eval)(patched); } finally { globalThis.Deno = sd; }
}"#,
        );
        let patched = patched.replace(
            "pyodide = await globalThis.loadPyodide();",
            concat!(
                "pyodide = await globalThis.loadPyodide({ ",
                "indexURL: new URL(PYODIDE_CDN, import.meta.url)",
                ".href.replace(/pyodide\\.js$/, '') });",
            ),
        );

        source.code = ModuleSourceCode::String(format!("{request_shim}{patched}").into());
        Ok(source)
    }
}

/// Injects browser polyfills into a rustyscript `Runtime` so that WASM modules
/// compiled for the browser can run in the native Deno-based V8 context.
///
/// Specifically:
/// - Defines a `Window` class with `Symbol.hasInstance` so that
///   `globalThis instanceof Window` passes (required by `web_sys::window()`)
///   without modifying the prototype chain (which would break pyodide).
/// - Sets `globalThis.window = globalThis` and `globalThis.location` so that
///   `websocket_url()` can derive the WS URL from `location.{protocol,host}`.
/// - Wraps `globalThis.fetch` to resolve root-relative URLs against `http_base`
///   (only effective for calls made via `globalThis.fetch`; native module-scope
///   `fetch` calls are handled by `RelativeUrlFixup`).
pub fn apply_browser_polyfills(runtime: &mut Runtime, http_base: &str) -> Result<(), rustyscript::Error> {
    let ws_protocol = if http_base.starts_with("https://") {
        "wss:"
    } else {
        "ws:"
    };
    let host = http_base.trim_start_matches("https://").trim_start_matches("http://");
    runtime.eval::<()>(format!(
        r#"
        globalThis.Window = function Window() {{}};
        Object.defineProperty(Window, Symbol.hasInstance, {{
            value: (inst) => inst === globalThis
        }});
        globalThis.WorkerGlobalScope = function WorkerGlobalScope() {{}};
        Object.defineProperty(WorkerGlobalScope, Symbol.hasInstance, {{
            value: (inst) => inst === globalThis
        }});
        globalThis.window = globalThis;
        globalThis.self = globalThis;
        globalThis.location = {{ protocol: "{ws_protocol}", host: "{host}", href: "{http_base}/",
            toString() {{ return this.href; }} }};
        // Stub document for modules that update DOM-like targets (e.g. log/output panels).
        // getElementById returns null so callers' `if (el) ...` guards work as written.
        // createElement('canvas') must return an HTMLCanvasElement instance so that
        // wasm-bindgen's `dyn_into::<HtmlCanvasElement>()` cast succeeds. The canvas
        // has a getContext method that returns null for unsupported context types,
        // letting WebGL/WebGL2 probes report "not supported" rather than crash.
        globalThis.HTMLCanvasElement = function HTMLCanvasElement() {{}};
        HTMLCanvasElement.prototype.getContext = function(_type) {{ return null; }};
        HTMLCanvasElement.prototype.width = 0;
        HTMLCanvasElement.prototype.height = 0;
        // navigator stub. rustyscript's `webgpu` feature loads deno_webgpu's Rust ops
        // but its init_webgpu.js leaves the JS-side wiring (navigator.gpu, GPU classes)
        // commented out, so navigator is undefined. Modules that probe `navigator.gpu`
        // see undefined and gracefully report "no WebGPU support". Real WebGPU wiring
        // would require a custom rustyscript extension that imports
        // `ext:deno_webgpu/01_webgpu.js` and assigns its `gpu` export to navigator.gpu.
        globalThis.navigator = globalThis.navigator || {{
            userAgent: "et-ws-worker/0.1 (rustyscript/deno_core)",
            gpu: null,
        }};
        globalThis.document = {{
            getElementById: () => null,
            createElement: (tag) => {{
                if (String(tag).toLowerCase() === "canvas") {{
                    return Object.create(HTMLCanvasElement.prototype);
                }}
                return {{ src: "", onload: null, onerror: null }};
            }},
            head: {{ appendChild: () => {{}} }},
        }};
        // globalThis.fetch is non-writable and non-configurable in deno_fetch, so we
        // can't replace it here. Module-scope shadowing (in RelativeUrlFixup) handles
        // relative-URL rewriting for `fetch('/path')` calls inside fetched modules.
        "#
    ))
}

/// Creates a `Runtime` configured for running browser-targeted WASM modules:
/// - `RelativeUrlFixup` as the import provider to patch relative URLs in fetched JS
/// - 60-second timeout
pub fn create_runtime(http_base: &str) -> Result<Runtime, rustyscript::Error> {
    Runtime::new(RuntimeOptions {
        timeout: Duration::from_secs(60),
        import_provider: Some(Box::new(RelativeUrlFixup {
            http_base: http_base.to_string(),
        })),
        extension_options: ExtensionOptions {
            web: WebOptions {
                base_url: rustyscript::deno_core::ModuleSpecifier::parse(http_base).ok(),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    })
}
