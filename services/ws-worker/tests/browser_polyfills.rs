//! Tests for the browser polyfill infrastructure in et_ws_worker.
//!
//! - `apply_browser_polyfills` (globalThis/Window/location/fetch setup)
//! - `RelativeUrlFixup` (ImportProvider that patches fetched JS modules)
//! - `create_runtime` (factory that wires both together)
//!
//! Several tests also document known Deno/rustyscript limitations that motivated
//! the post_process approach over globalThis property assignment.

#![cfg(test)]

use std::time::Duration;

use et_ws_worker::{RelativeUrlFixup, apply_browser_polyfills, create_runtime};
use rustyscript::module_loader::ImportProvider;
use rustyscript::{Module, Runtime, RuntimeOptions, json_args};

fn default_runtime() -> Runtime {
    Runtime::new(RuntimeOptions {
        timeout: Duration::from_secs(10),
        ..Default::default()
    })
    .unwrap()
}

// ── apply_browser_polyfills ────────────────────────────

#[test]
fn polyfills_make_window_visible_as_free_variable() {
    let mut rt = default_runtime();
    apply_browser_polyfills(&mut rt, "http://localhost:8080").unwrap();

    let m = Module::new("t.js", "export const t = typeof window;");
    let h = rt.load_module(&m).unwrap();
    let t: String = rt.get_value(Some(&h), "t").unwrap();
    assert_eq!(t, "object");
}

#[test]
fn polyfills_set_location_protocol_and_host() {
    let mut rt = default_runtime();
    apply_browser_polyfills(&mut rt, "http://localhost:8080").unwrap();

    let m = Module::new(
        "t.js",
        "export const proto = location.protocol; export const host = location.host;",
    );
    let h = rt.load_module(&m).unwrap();
    let proto: String = rt.get_value(Some(&h), "proto").unwrap();
    let host: String = rt.get_value(Some(&h), "host").unwrap();
    assert_eq!(proto, "ws:");
    assert_eq!(host, "localhost:8080");
}

#[test]
fn polyfills_set_location_wss_for_https_base() {
    let mut rt = default_runtime();
    apply_browser_polyfills(&mut rt, "https://example.com").unwrap();

    let m = Module::new("t.js", "export const proto = location.protocol;");
    let h = rt.load_module(&m).unwrap();
    let proto: String = rt.get_value(Some(&h), "proto").unwrap();
    assert_eq!(proto, "wss:");
}

#[test]
fn polyfills_make_globalthis_instanceof_window() {
    let mut rt = default_runtime();
    apply_browser_polyfills(&mut rt, "http://localhost:8080").unwrap();

    let m = Module::new("t.js", "export const ok = globalThis instanceof Window;");
    let h = rt.load_module(&m).unwrap();
    let ok: bool = rt.get_value(Some(&h), "ok").unwrap();
    assert!(ok, "globalThis instanceof Window must be true for web_sys::window()");
}

#[test]
fn polyfills_preserve_existing_globalthis_methods() {
    let mut rt = default_runtime();
    apply_browser_polyfills(&mut rt, "http://localhost:8080").unwrap();

    // dispatchEvent is provided by deno_web; must survive the prototype swap.
    let m = Module::new(
        "t.js",
        "export const ok = typeof globalThis.dispatchEvent === 'function';",
    );
    let h = rt.load_module(&m).unwrap();
    let ok: bool = rt.get_value(Some(&h), "ok").unwrap();
    assert!(ok, "dispatchEvent must still be available after Window polyfill");
}

#[test]
fn polyfills_persist_into_async_block_on() {
    let mut rt = default_runtime();
    apply_browser_polyfills(&mut rt, "http://localhost:8080").unwrap();

    let m = Module::new("t.js", r#"export async function check() { return location.host; }"#);
    let h = rt.load_module(&m).unwrap();
    let tokio = rt.tokio_runtime();
    let host: String = tokio
        .block_on(async { rt.call_function_async(Some(&h), "check", json_args!()).await })
        .unwrap();
    assert_eq!(host, "localhost:8080");
}

// ── RelativeUrlFixup (post_process) ─────────────────────────

#[test]
fn relative_url_fixup_skips_non_http_modules() {
    use rustyscript::deno_core::{ModuleSource, ModuleSourceCode, ModuleType};

    let mut fixup = RelativeUrlFixup {
        http_base: "http://localhost:8080".to_string(),
    };
    let specifier = rustyscript::deno_core::ModuleSpecifier::parse("file:///local.js").unwrap();
    let source = ModuleSource::new(
        ModuleType::JavaScript,
        ModuleSourceCode::String("export const x = 1;".to_string().into()),
        &specifier,
        None,
    );
    let result = fixup.post_process(&specifier, source).unwrap();
    let code = String::from_utf8_lossy(result.code.as_bytes()).into_owned();
    assert!(!code.contains("var Request"), "non-http modules must not be patched");
}

#[test]
fn relative_url_fixup_prepends_request_shim_to_http_modules() {
    use rustyscript::deno_core::{ModuleSource, ModuleSourceCode, ModuleType};

    let mut fixup = RelativeUrlFixup {
        http_base: "http://localhost:8080".to_string(),
    };
    let specifier = rustyscript::deno_core::ModuleSpecifier::parse("http://localhost:8080/mod.js").unwrap();
    let source = ModuleSource::new(
        ModuleType::JavaScript,
        ModuleSourceCode::String("export const x = 1;".to_string().into()),
        &specifier,
        None,
    );
    let result = fixup.post_process(&specifier, source).unwrap();
    let code = String::from_utf8_lossy(result.code.as_bytes()).into_owned();
    assert!(code.trim().starts_with("var Request"), "shim must be prepended");
    assert!(code.contains("http://localhost:8080"), "base URL must appear in shim");
    assert!(
        code.contains("export const x = 1;"),
        "original source must be preserved"
    );
}

#[test]
fn relative_url_fixup_rewrites_wasm_fetch_call_site() {
    use rustyscript::deno_core::{ModuleSource, ModuleSourceCode, ModuleType};

    let mut fixup = RelativeUrlFixup {
        http_base: "http://localhost:8080".to_string(),
    };
    let specifier = rustyscript::deno_core::ModuleSpecifier::parse("http://localhost:8080/mod.js").unwrap();
    let wasm_glue = "const ret = arg0.fetch(getStringFromWasm0(arg1, arg2));";
    let source = ModuleSource::new(
        ModuleType::JavaScript,
        ModuleSourceCode::String(wasm_glue.to_string().into()),
        &specifier,
        None,
    );
    let result = fixup.post_process(&specifier, source).unwrap();
    let code = String::from_utf8_lossy(result.code.as_bytes()).into_owned();
    assert!(
        !code.contains("const ret = arg0.fetch(getStringFromWasm0(arg1, arg2));"),
        "original fetch call site must be replaced"
    );
    assert!(
        code.contains("__u[0] === '/'"),
        "patched code must include relative-URL check"
    );
}

// ── create_runtime ───────────────────────────────

#[test]
fn create_runtime_returns_working_runtime() {
    let mut rt = create_runtime("http://localhost:8080").unwrap();
    let val: i32 = rt.eval("2 + 2").unwrap();
    assert_eq!(val, 4);
}

// ── Known Deno/rustyscript limitations (documented) ──────────────

/// Native `Request` binding in ES module scope cannot be overridden via
/// `globalThis.Request` — the fix is `RelativeUrlFixup::post_process`.
#[test]
fn request_native_binding_cannot_be_overridden_via_globalthis() {
    let mut rt = default_runtime();
    rt.eval::<()>(
        r#"
        const _O = globalThis.Request;
        globalThis.Request = function(i,n){ return new _O(i,n); };
        "#,
    )
    .unwrap();

    let m = Module::new(
        "t.js",
        r#"export async function f() {
            const r = new Request('/x', { method: 'PUT' });
            return r.url;
        }"#,
    );
    let h = rt.load_module(&m).unwrap();
    let tokio = rt.tokio_runtime();
    let result: Result<String, _> = tokio.block_on(async { rt.call_function_async(Some(&h), "f", json_args!()).await });
    assert!(result.is_err(), "native Request binding bypasses globalThis override");
}

/// Native `fetch` binding is non-writable; `globalThis.fetch` assignment is silently
/// ignored. The fix is source-rewriting in `RelativeUrlFixup::post_process`.
#[test]
fn fetch_native_binding_cannot_be_overridden_via_globalthis() {
    let mut rt = default_runtime();
    rt.eval::<()>(
        r#"
        const _f = globalThis.fetch;
        globalThis._log = [];
        globalThis.fetch = function(i,n){ globalThis._log.push(i); return _f(i,n); };
        "#,
    )
    .unwrap();

    let m = Module::new(
        "t.js",
        r#"export async function f() {
            try { await window.fetch('/test'); } catch(_) {}
            return globalThis._log;
        }"#,
    );
    let h = rt.load_module(&m).unwrap();
    let tokio = rt.tokio_runtime();
    let log: Vec<String> = tokio
        .block_on(async { rt.call_function_async(Some(&h), "f", json_args!()).await })
        .unwrap();
    assert!(
        log.is_empty(),
        "native fetch binding bypasses globalThis.fetch override"
    );
}

#[test]
fn polyfills_make_self_instanceof_worker_global_scope() {
    use std::time::Duration;

    use rustyscript::{Module, Runtime, RuntimeOptions};
    let mut rt = Runtime::new(RuntimeOptions {
        timeout: Duration::from_secs(5),
        ..Default::default()
    })
    .unwrap();
    apply_browser_polyfills(&mut rt, "http://localhost:8080").unwrap();
    let m = Module::new(
        "t.js",
        r#"
        export const inWorker = (
            typeof globalThis.WorkerGlobalScope !== 'undefined' &&
            typeof globalThis.self !== 'undefined' &&
            globalThis.self instanceof globalThis.WorkerGlobalScope
        );
    "#,
    );
    let h = rt.load_module(&m).unwrap();
    let v: bool = rt.get_value(Some(&h), "inWorker").unwrap();
    assert!(
        v,
        "self instanceof WorkerGlobalScope must be true for pyodide IN_BROWSER_WEB_WORKER"
    );
}

#[test]
fn polyfills_visible_in_dynamically_imported_module() {
    use std::time::Duration;

    use rustyscript::{Module, Runtime, RuntimeOptions, json_args};
    let mut rt = Runtime::new(RuntimeOptions {
        timeout: Duration::from_secs(5),
        ..Default::default()
    })
    .unwrap();
    apply_browser_polyfills(&mut rt, "http://localhost:8080").unwrap();
    // inner.js checks the polyfills
    let inner = Module::new(
        "inner.js",
        r#"
        export const inWorker = (
            typeof globalThis.WorkerGlobalScope !== 'undefined' &&
            globalThis.self instanceof globalThis.WorkerGlobalScope
        );
    "#,
    );
    // outer.js dynamically imports inner.js
    let outer = Module::new(
        "outer.js",
        r#"
        export async function check() {
            const m = await import("./inner.js");
            return m.inWorker;
        }
    "#,
    );
    let h = rt.load_modules(&outer, vec![&inner]).unwrap();
    let tokio = rt.tokio_runtime();
    let v: bool = tokio
        .block_on(async { rt.call_function_async(Some(&h), "check", json_args!()).await })
        .unwrap();
    assert!(
        v,
        "WorkerGlobalScope polyfill must be visible in dynamically imported modules"
    );
}

#[test]
fn deno_global_not_exposed_in_rustyscript() {
    use std::time::Duration;

    use rustyscript::{Module, Runtime, RuntimeOptions};
    let mut rt = Runtime::new(RuntimeOptions {
        timeout: Duration::from_secs(5),
        ..Default::default()
    })
    .unwrap();
    let m = Module::new("t.js", "export const hasDeno = typeof Deno !== 'undefined';");
    let h = rt.load_module(&m).unwrap();
    let v: bool = rt.get_value(Some(&h), "hasDeno").unwrap();
    // If Deno IS exposed, pyodide will try to use Deno-specific APIs and fail.
    println!("Deno exposed: {v}");
}

/// Verify that post_process is called for modules loaded via dynamic import().
#[test]
fn post_process_called_for_dynamic_imports() {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;


    use rustyscript::module_loader::ImportProvider;
    use rustyscript::{Module, Runtime, RuntimeOptions, json_args};

    // Track which specifiers post_process is called for
    let visited = Arc::new(Mutex::new(vec![]));
    let visited_clone = visited.clone();

    struct TrackingProvider {
        visited: Arc<Mutex<Vec<String>>>,
    }
    impl ImportProvider for TrackingProvider {
        fn post_process(
            &mut self,
            specifier: &rustyscript::deno_core::ModuleSpecifier,
            source: rustyscript::deno_core::ModuleSource,
        ) -> Result<rustyscript::deno_core::ModuleSource, rustyscript::deno_core::error::ModuleLoaderError> {
            self.visited.lock().unwrap().push(specifier.to_string());
            Ok(source)
        }
    }

    let mut rt = Runtime::new(RuntimeOptions {
        timeout: Duration::from_secs(5),
        import_provider: Some(Box::new(TrackingProvider { visited: visited_clone })),
        ..Default::default()
    })
    .unwrap();

    let inner = Module::new("inner.js", "export const x = 42;");
    let outer = Module::new(
        "outer.js",
        r#"
        export async function load() {
            const m = await import("./inner.js");
            return m.x;
        }
    "#,
    );
    let h = rt.load_modules(&outer, vec![&inner]).unwrap();
    let tokio = rt.tokio_runtime();
    let _: i32 = tokio
        .block_on(async { rt.call_function_async(Some(&h), "load", json_args!()).await })
        .unwrap();

    let seen = visited.lock().unwrap().clone();
    println!("post_process called for: {:?}", seen);
    // post_process is NOT called for dynamic imports — only for load_module/load_modules.
    // This is a known rustyscript limitation: pyodide.js loaded via import() inside
    // loadPyodideScript cannot be patched via post_process; instead we patch the
    // loadPyodideScript source itself (via post_process on et_ws_pydata1.js) to apply
    // the pyodide patches inline using fetch+eval.
    assert!(
        !seen.iter().any(|s| s.contains("inner.js")),
        "post_process must NOT be called for dynamically imported modules (known limitation), got: {:?}",
        seen
    );
}

/// Verify the loadPyodideScript pattern we're patching actually exists in et_ws_pydata1.js.
#[test]
fn pydata1_js_contains_load_pyodide_script_pattern() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../services/ws-modules/pydata1/pkg/et_ws_pydata1.js"
    );
    let content = std::fs::read_to_string(path).expect("et_ws_pydata1.js not found");
    assert!(
        content.contains("document.createElement"),
        "loadPyodideScript uses document.createElement"
    );
    assert!(
        content.contains("loadPyodideScript"),
        "function loadPyodideScript exists"
    );
}

/// Verify RelativeUrlFixup patches et_ws_pydata1.js to replace loadPyodideScript.
#[test]
fn relative_url_fixup_patches_load_pyodide_script() {
    use et_ws_worker::RelativeUrlFixup;
    use rustyscript::deno_core::{ModuleSource, ModuleSourceCode, ModuleType};
    use rustyscript::module_loader::ImportProvider;

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../services/ws-modules/pydata1/pkg/et_ws_pydata1.js"
    );
    let content = std::fs::read_to_string(path).expect("et_ws_pydata1.js not found");

    let mut fixup = RelativeUrlFixup {
        http_base: "http://localhost:8080".to_string(),
    };
    let specifier =
        rustyscript::deno_core::ModuleSpecifier::parse("http://localhost:8080/modules/et-ws-pydata1/et_ws_pydata1.js")
            .unwrap();
    let source = ModuleSource::new(
        ModuleType::JavaScript,
        ModuleSourceCode::String(content.into()),
        &specifier,
        None,
    );
    let result = fixup.post_process(&specifier, source).unwrap();
    let code = String::from_utf8_lossy(result.code.as_bytes()).into_owned();

    assert!(
        !code.contains("document.createElement"),
        "document.createElement must be removed"
    );
    assert!(code.contains("fetch(url)"), "patched version must use fetch");
    assert!(code.contains("(0, eval)(patched)"), "patched version must use eval");
}

/// Verify the pyodide.js patch string exists in the actual pyodide.js file.
#[test]
fn pyodide_js_contains_cannot_determine_runtime_env_string() {
    // Find pyodide.js via mise
    let output = std::process::Command::new("mise")
        .args(["where", "npm:pyodide"])
        .output()
        .expect("mise not found");
    let base = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let pyodide_js = format!("{base}/lib/node_modules/pyodide/pyodide.js");
    let content =
        std::fs::read_to_string(&pyodide_js).unwrap_or_else(|_| panic!("pyodide.js not found at {pyodide_js}"));

    let target = concat!(
        r#"else if(f.IN_NODE)k=Ne;else if(f.IN_SHELL)k=load;"#,
        r#"else throw new Error("Cannot determine runtime environment")"#,
    );
    assert!(
        content.contains(target),
        "pyodide.js must contain the patch target string"
    );
}

/// Verify that fetch+eval of pyodide.js (with the loadScript patch applied inline)
/// successfully sets globalThis.loadPyodide.
#[test]
fn fetch_eval_pyodide_js_with_inline_patch_sets_load_pyodide() {
    use std::time::Duration;

    use rustyscript::{Module, Runtime, RuntimeOptions};

    // Find pyodide.js
    let output = std::process::Command::new("mise")
        .args(["where", "npm:pyodide"])
        .output()
        .expect("mise not found");
    let base = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let pyodide_js = format!("{base}/lib/node_modules/pyodide/pyodide.js");
    let content =
        std::fs::read_to_string(&pyodide_js).unwrap_or_else(|_| panic!("pyodide.js not found at {pyodide_js}"));

    // Apply the same patch we do in loadPyodideScript
    let target = concat!(
        r#"else if(f.IN_NODE)k=Ne;else if(f.IN_SHELL)k=load;"#,
        r#"else throw new Error("Cannot determine runtime environment")"#,
    );
    let replacement = concat!(
        r#"else if(f.IN_NODE)k=Ne;else if(f.IN_SHELL)k=load;"#,
        r#"else k=async function(e){const t=await(await fetch(e)).text();(0,eval)(t)}"#,
    );
    let patched = content.replace(target, replacement);
    assert!(patched.contains(replacement), "patch must be applied");

    let mut rt = Runtime::new(RuntimeOptions {
        timeout: Duration::from_secs(10),
        ..Default::default()
    })
    .unwrap();

    // Eval the patched pyodide.js directly
    rt.eval::<()>(patched).unwrap();

    // Check that loadPyodide is now set
    let m = Module::new(
        "t.js",
        "export const has = typeof globalThis.loadPyodide === 'function';",
    );
    let h = rt.load_module(&m).unwrap();
    let has: bool = rt.get_value(Some(&h), "has").unwrap();
    assert!(
        has,
        "globalThis.loadPyodide must be set after evaling patched pyodide.js"
    );
}

/// Verify the inline JS string replacement in the patched loadPyodideScript
/// correctly patches pyodide.js before eval-ing it.
#[test]
fn patched_load_pyodide_script_inline_replacement_works() {
    use et_ws_worker::{apply_browser_polyfills, create_runtime};
    use rustyscript::Module;

    // Find pyodide.js path via mise
    let output = std::process::Command::new("mise")
        .args(["where", "npm:pyodide"])
        .output()
        .expect("mise not found");
    let base = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let _pyodide_url = "http://localhost:9999/modules/pyodide/pyodide.js".to_string();

    // Read and serve pyodide.js content directly (simulate what the server would serve)
    let pyodide_path = format!("{base}/lib/node_modules/pyodide/pyodide.js");
    let pyodide_content = std::fs::read_to_string(&pyodide_path).unwrap();

    let mut rt = create_runtime("http://localhost:9999").unwrap();
    apply_browser_polyfills(&mut rt, "http://localhost:9999").unwrap();

    // Simulate what the patched loadPyodideScript does:
    // 1. fetch pyodide.js text (we inject it directly)
    // 2. apply the inline string replacement
    // 3. eval the result
    let target = concat!(
        r#"else if(f.IN_NODE)k=Ne;else if(f.IN_SHELL)k=load;"#,
        r#"else throw new Error("Cannot determine runtime environment")"#,
    );
    let replacement = concat!(
        r#"else if(f.IN_NODE)k=Ne;else if(f.IN_SHELL)k=load;"#,
        r#"else k=async function(e){const t=await(await fetch(e)).text();(0,eval)(t)}"#,
    );
    let patched_pyodide = pyodide_content.replace(target, replacement);

    // Inject the patched pyodide.js text as a global, then eval it
    rt.eval::<()>(format!(
        "globalThis.__pyodideText = {};",
        serde_json::to_string(&patched_pyodide).unwrap()
    ))
    .unwrap();
    rt.eval::<()>("(0, eval)(globalThis.__pyodideText);").unwrap();

    // Check loadPyodide is now available
    let m = Module::new(
        "t.js",
        "export const has = typeof globalThis.loadPyodide === 'function';",
    );
    let h = rt.load_module(&m).unwrap();
    let has: bool = rt.get_value(Some(&h), "has").unwrap();
    assert!(has, "globalThis.loadPyodide must be set after inline-patched eval");
}

/// Print the actual patched loadPyodideScript to inspect the generated JS.
#[test]
fn inspect_patched_load_pyodide_script_js() {
    use et_ws_worker::RelativeUrlFixup;
    use rustyscript::deno_core::{ModuleSource, ModuleSourceCode, ModuleType};
    use rustyscript::module_loader::ImportProvider;

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../services/ws-modules/pydata1/pkg/et_ws_pydata1.js"
    );
    let content = std::fs::read_to_string(path).unwrap();
    let mut fixup = RelativeUrlFixup {
        http_base: "http://localhost:8080".to_string(),
    };
    let specifier =
        rustyscript::deno_core::ModuleSpecifier::parse("http://localhost:8080/modules/et-ws-pydata1/et_ws_pydata1.js")
            .unwrap();
    let source = ModuleSource::new(
        ModuleType::JavaScript,
        ModuleSourceCode::String(content.into()),
        &specifier,
        None,
    );
    let result = fixup.post_process(&specifier, source).unwrap();
    let code = String::from_utf8_lossy(result.code.as_bytes()).into_owned();

    // Print the loadPyodideScript function from the patched code
    if let Some(start) = code.find("async function loadPyodideScript") {
        let end = code[start..].find("\n}").map(|i| start + i + 2).unwrap_or(start + 500);
        println!("Patched loadPyodideScript:\n{}", &code[start..end.min(code.len())]);
    }
}

/// End-to-end test: run the patched loadPyodideScript against a real HTTP server
/// serving pyodide.js, and verify globalThis.loadPyodide is set.
#[test]
#[ignore]
fn patched_load_pyodide_script_runs_against_http_server() {
    use et_ws_test_server::start;
    use et_ws_worker::{apply_browser_polyfills, create_runtime};


    use rustyscript::{Module, json_args};

    let server = start();
    let base = &server.base_url;

    let mut rt = create_runtime(base).unwrap();
    apply_browser_polyfills(&mut rt, base).unwrap();

    // Load the patched et_ws_pydata1.js (post_process will patch it)
    let entry_url = format!("{base}/modules/et-ws-pydata1/et_ws_pydata1.js");
    let stub = rustyscript::Module::new("entry.js", format!(r#"export {{ default }} from {entry_url:?};"#));
    let handle = rt.load_module(&stub).unwrap();

    // Call default() which runs loadPyodideScript() then loadPyodide()
    let tokio = rt.tokio_runtime();
    let result: Result<rustyscript::Undefined, _> = tokio.block_on(async {
        rt.call_function_async::<rustyscript::Undefined>(Some(&handle), "default", json_args!())
            .await
    });

    match &result {
        Ok(_) => println!("default() succeeded"),
        Err(e) => println!("default() failed: {e}"),
    }

    // Check if loadPyodide was set (even if init failed partway through)
    let m = Module::new(
        "check.js",
        "export const has = typeof globalThis.loadPyodide === 'function';",
    );
    let h = rt.load_module(&m).unwrap();
    let has: bool = rt.get_value(Some(&h), "has").unwrap();
    assert!(has, "globalThis.loadPyodide must be set after loadPyodideScript runs");
}

/// Check if _createPyodideModule is set after pyodide.asm.js is loaded.
#[test]
#[ignore]
fn pyodide_asm_js_sets_create_pyodide_module() {
    use et_ws_test_server::start;
    use et_ws_worker::{apply_browser_polyfills, create_runtime};
    use rustyscript::{Module, json_args};

    let server = start();
    let base = &server.base_url;
    let mut rt = create_runtime(base).unwrap();
    apply_browser_polyfills(&mut rt, base).unwrap();

    let entry_url = format!("{base}/modules/et-ws-pydata1/et_ws_pydata1.js");
    let stub = rustyscript::Module::new("entry.js", format!(r#"export {{ default }} from {entry_url:?};"#));
    let handle = rt.load_module(&stub).unwrap();

    let tokio = rt.tokio_runtime();
    let _ = tokio.block_on(async {
        rt.call_function_async::<rustyscript::Undefined>(Some(&handle), "default", json_args!())
            .await
    });

    let m = Module::new(
        "check.js",
        r#"
        export const hasCreate = typeof globalThis._createPyodideModule === 'function';
        export const hasLoadPyodide = typeof globalThis.loadPyodide === 'function';
    "#,
    );
    let h = rt.load_module(&m).unwrap();
    let has_create: bool = rt.get_value(Some(&h), "hasCreate").unwrap();
    let has_load: bool = rt.get_value(Some(&h), "hasLoadPyodide").unwrap();
    println!("_createPyodideModule: {has_create}, loadPyodide: {has_load}");
    assert!(has_load, "loadPyodide must be set");
    // _createPyodideModule may or may not be set depending on how pyodide.asm.js loads
}

/// Check if Symbol.hasInstance can make globalThis instanceof Window work
/// without modifying the prototype chain (which breaks pyodide).
#[test]
fn window_instanceof_via_symbol_has_instance() {
    use std::time::Duration;

    use rustyscript::{Module, Runtime, RuntimeOptions};
    let mut rt = Runtime::new(RuntimeOptions {
        timeout: Duration::from_secs(5),
        ..Default::default()
    })
    .unwrap();
    rt.eval::<()>(r#"
        globalThis.Window = function Window() {};
        Object.defineProperty(Window, Symbol.hasInstance, {
            value: function(instance) { return instance === globalThis; }
        });
        globalThis.window = globalThis;
        globalThis.location = { protocol: "ws:", host: "localhost:8080",
            href: "http://localhost:8080/", toString() { return this.href; } };
        globalThis.self = globalThis;
    "#).unwrap();
    let m = Module::new(
        "t.js",
        r#"
        export const isWindow = globalThis instanceof Window;
        export const protoOk = Object.getPrototypeOf(globalThis) !== null;
    "#,
    );
    let h = rt.load_module(&m).unwrap();
    let is_window: bool = rt.get_value(Some(&h), "isWindow").unwrap();
    let proto_ok: bool = rt.get_value(Some(&h), "protoOk").unwrap();
    println!("instanceof Window: {is_window}, prototype chain intact: {proto_ok}");
    assert!(is_window, "globalThis instanceof Window must be true");
    assert!(proto_ok, "prototype chain must remain intact");
}

/// Run loadPyodide() fully and report the exact error.
#[test]
fn load_pyodide_full_run() {
    use et_ws_test_server::start;
    use et_ws_worker::{apply_browser_polyfills, create_runtime};
    use rustyscript::json_args;

    let server = start();
    let base = &server.base_url;
    let mut rt = create_runtime(base).unwrap();
    apply_browser_polyfills(&mut rt, base).unwrap();

    let entry_url = format!("{base}/modules/et-ws-pydata1/et_ws_pydata1.js");
    let stub = rustyscript::Module::new("entry.js", format!(r#"export {{ default }} from {entry_url:?};"#));
    let handle = rt.load_module(&stub).unwrap();

    let tokio = rt.tokio_runtime();
    let result = tokio.block_on(async {
        rt.call_function_async::<rustyscript::Undefined>(Some(&handle), "default", json_args!())
            .await
    });
    match result {
        Ok(_) => println!("loadPyodide succeeded!"),
        Err(e) => println!("loadPyodide failed: {e}"),
    }
}

/// Test pyodide loading by directly eval-ing pyodide.js and pyodide.asm.js
/// without going through the HTTP server, to isolate the issue.
#[test]
#[ignore]
fn pyodide_loads_via_direct_eval() {
    use et_ws_test_server::start;
    use et_ws_worker::{apply_browser_polyfills, create_runtime};
    use rustyscript::{Module, json_args};

    let server = start();
    let base = &server.base_url;

    // Find pyodide files
    let output = std::process::Command::new("mise")
        .args(["where", "npm:pyodide"])
        .output()
        .unwrap();
    let mise_base = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let pyodide_dir = format!("{mise_base}/lib/node_modules/pyodide");

    let pyodide_js = std::fs::read_to_string(format!("{pyodide_dir}/pyodide.js")).unwrap();
    let pyodide_asm_js = std::fs::read_to_string(format!("{pyodide_dir}/pyodide.asm.js")).unwrap();

    let patch = |src: &str| -> String {
        src.replace(
            r#"else throw new Error("Cannot determine runtime environment")"#,
            r#"else k=async function(e){const t=await(await fetch(e)).text();(0,eval)(globalThis.__patchScript(t))}"#,
        )
    };

    let mut rt = create_runtime(base).unwrap();
    apply_browser_polyfills(&mut rt, base).unwrap();

    // Set up __patchScript globally
    rt.eval::<()>(
        r#"globalThis.__patchScript = (src) => src.replace(
        /else throw new Error\("Cannot determine runtime environment"\)/g,
        'else k=async function(e){const t=await(await fetch(e)).text();(0,eval)(globalThis.__patchScript(t))}'
    );"#,
    )
    .unwrap();

    // Eval patched pyodide.js directly (no HTTP)
    rt.eval::<()>(patch(&pyodide_js)).unwrap();

    // Eval patched pyodide.asm.js directly (no HTTP)
    rt.eval::<()>(patch(&pyodide_asm_js)).unwrap();

    // Now try loadPyodide with the indexURL pointing to the server
    let index_url = format!("{base}/modules/pyodide/");
    let m = Module::new(
        "t.js",
        format!(
            r#"
        export async function go() {{
            return await globalThis.loadPyodide({{ indexURL: {index_url:?} }});
        }}
    "#
        ),
    );
    let h = rt.load_module(&m).unwrap();
    let tokio = rt.tokio_runtime();
    let result = tokio.block_on(async {
        rt.call_function_async::<rustyscript::Undefined>(Some(&h), "go", json_args!())
            .await
    });
    match result {
        Ok(_) => println!("loadPyodide via direct eval succeeded!"),
        Err(e) => println!(
            "loadPyodide via direct eval failed: {}",
            &e.to_string()[..e.to_string().len().min(300)]
        ),
    }
}

/// Minimal pyodide test: load pyodide and run a trivial Python expression.
#[test]
#[ignore]
fn pyodide_runs_python() {
    use et_ws_test_server::start;
    use et_ws_worker::{apply_browser_polyfills, create_runtime};
    use rustyscript::{Module, json_args};

    let server = start();
    let base = &server.base_url;

    let output = std::process::Command::new("mise")
        .args(["where", "npm:pyodide"])
        .output()
        .unwrap();
    let mise_base = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let pyodide_dir = format!("{mise_base}/lib/node_modules/pyodide");
    let pyodide_js = std::fs::read_to_string(format!("{pyodide_dir}/pyodide.js")).unwrap();
    let pyodide_asm_js = std::fs::read_to_string(format!("{pyodide_dir}/pyodide.asm.js")).unwrap();

    let patch = |src: &str| {
        src.replace(
            r#"else throw new Error("Cannot determine runtime environment")"#,
            r#"else k=async function(e){const t=await(await fetch(e)).text();(0,eval)(globalThis.__patchScript(t))}"#,
        )
    };

    let mut rt = create_runtime(base).unwrap();
    apply_browser_polyfills(&mut rt, base).unwrap();
    rt.eval::<()>(
        r#"globalThis.__patchScript = (src) => src.replace(
        /else throw new Error\("Cannot determine runtime environment"\)/g,
        'else k=async function(e){const t=await(await fetch(e)).text();(0,eval)(globalThis.__patchScript(t))}'
    );"#,
    )
    .unwrap();
    rt.eval::<()>(patch(&pyodide_js)).unwrap();
    rt.eval::<()>(patch(&pyodide_asm_js)).unwrap();

    let index_url = format!("{base}/modules/pyodide/");
    let m = Module::new(
        "t.js",
        format!(
            r#"
        export async function go() {{
            const py = await globalThis.loadPyodide({{ indexURL: {index_url:?} }});
            return py.runPython("1 + 1");
        }}
    "#
        ),
    );
    let h = rt.load_module(&m).unwrap();
    let tokio = rt.tokio_runtime();
    let result = tokio.block_on(async { rt.call_function_async::<i32>(Some(&h), "go", json_args!()).await });
    match result {
        Ok(v) => {
            println!("Python 1+1 = {v}");
            assert_eq!(v, 2);
        }
        Err(e) => {
            let msg = e.to_string();
            println!("Failed: {}", &msg[..msg.len().min(500)]);
            panic!("pyodide failed");
        }
    }
}

/// Test if polyfilling node:vm.runInThisContext makes pyodide use the Deno path successfully.
#[ignore]
#[test]
fn pyodide_with_node_vm_polyfill() {
    use et_ws_test_server::start;
    use et_ws_worker::{apply_browser_polyfills, create_runtime};
    use rustyscript::{Module, json_args};

    let server = start();
    let base = &server.base_url;

    let output = std::process::Command::new("mise")
        .args(["where", "npm:pyodide"])
        .output()
        .unwrap();
    let mise_base = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let pyodide_dir = format!("{mise_base}/lib/node_modules/pyodide");
    let pyodide_js = std::fs::read_to_string(format!("{pyodide_dir}/pyodide.js")).unwrap();

    let mut rt = create_runtime(base).unwrap();
    apply_browser_polyfills(&mut rt, base).unwrap();

    // Polyfill node:vm so pyodide's Deno/Node path works.
    // pyodide uses J.runInThisContext(code) to load pyodide.asm.js.
    // We implement it as (0, eval)(code).
    rt.eval::<()>(
        r#"
        globalThis.__nodeVm = { runInThisContext: (code) => (0, eval)(code) };
    "#,
    )
    .unwrap();

    // Patch pyodide.js: replace the node:vm import with our polyfill,
    // and replace the Deno loadScript (Ne) to use fetch + runInThisContext.
    let node_imports_pattern = r#"J=(await import(/* webpackIgnore */"node:url")).default,
V=await import(/* webpackIgnore */"node:fs"),U=await import(/* webpackIgnore */"node:fs/promises"),
J=(await import(/* webpackIgnore */"node:vm")).default"#;
    let patched_pyodide = pyodide_js
        // Replace: J=(await import("node:vm")).default  ->  J=globalThis.__nodeVm
        .replace(
            node_imports_pattern,
            r#"J=globalThis.__nodeVm,V={},U={readFile:async()=>{throw new Error("no fs")}}"#,
        );

    rt.eval::<()>(patched_pyodide).unwrap();

    let index_url = format!("{base}/modules/pyodide/");
    let m = Module::new(
        "t.js",
        format!(
            r#"
        export async function go() {{
            const py = await globalThis.loadPyodide({{ indexURL: {index_url:?} }});
            return py.runPython("1 + 1");
        }}
    "#
        ),
    );
    let h = rt.load_module(&m).unwrap();
    let tokio = rt.tokio_runtime();
    let result = tokio.block_on(async { rt.call_function_async::<i32>(Some(&h), "go", json_args!()).await });
    match result {
        Ok(v) => {
            println!("Python 1+1 = {v}");
            assert_eq!(v, 2);
        }
        Err(e) => {
            let msg = e.to_string();
            println!("Failed: {}", &msg[..msg.len().min(400)]);
            panic!("pyodide with node:vm polyfill failed");
        }
    }
}

/// Test pyodide without any Window polyfill to isolate the getPyProxyClass failure.
#[ignore]
#[test]
fn pyodide_without_window_polyfill() {
    use et_ws_test_server::start;
    use et_ws_worker::create_runtime;
    use rustyscript::{Module, json_args};

    let server = start();
    let base = &server.base_url;

    let output = std::process::Command::new("mise")
        .args(["where", "npm:pyodide"])
        .output()
        .unwrap();
    let mise_base = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let pyodide_dir = format!("{mise_base}/lib/node_modules/pyodide");
    let pyodide_js = std::fs::read_to_string(format!("{pyodide_dir}/pyodide.js")).unwrap();

    let mut rt = create_runtime(base).unwrap();
    // Only set location, no Window polyfill
    rt.eval::<()>(&format!(r#"
        globalThis.location = {{ protocol: "ws:", host: "127.0.0.1",
            href: "{base}/", toString() {{ return this.href; }} }};
        globalThis.__pyEval = async function(e) {{
            const t = await (await fetch(e)).text();
            const p = t
                .replace(/J\.runInThisContext\(/g, '(0,eval)(')
                .replace(
                    /else throw new Error\("Cannot determine runtime environment"\)/g,
                    'else k=async function(u){{return globalThis.__pyEval(u)}}'
                );
            (0, eval)(p);
        }};
    "#)).unwrap();

    let patched_pyodide = pyodide_js.replace("J.runInThisContext(", "(0,eval)(").replace(
        r#"else throw new Error("Cannot determine runtime environment")"#,
        "else k=async function(u){return globalThis.__pyEval(u)}",
    );
    rt.eval::<()>(&patched_pyodide).unwrap();

    let index_url = format!("{base}/modules/pyodide/");
    let m = Module::new(
        "t.js",
        format!(
            r#"
        export async function go() {{
            const py = await globalThis.loadPyodide({{ indexURL: {index_url:?} }});
            return py.runPython("1 + 1");
        }}
    "#
        ),
    );
    let h = rt.load_module(&m).unwrap();
    let tokio = rt.tokio_runtime();
    let result = tokio.block_on(async { rt.call_function_async::<i32>(Some(&h), "go", json_args!()).await });
    match result {
        Ok(v) => {
            println!("Python 1+1 = {v}");
            assert_eq!(v, 2);
        }
        Err(e) => println!(
            "Failed (no Window polyfill): {}",
            &e.to_string()[..e.to_string().len().min(200)]
        ),
    }
}

/// Probe what rustyscript provides without polyfills (with the `web` + `webgpu`
/// features enabled). Used to determine whether navigator/navigator.gpu need
/// polyfilling for graphics-info.
#[test]
fn navigator_and_webgpu_globals_present_with_features() {
    let mut rt = default_runtime();
    let m = Module::new(
        "p.js",
        r#"
        export const navType = typeof globalThis.navigator;
        export const gpuType = typeof globalThis.navigator?.gpu;
        export const ua = globalThis.navigator?.userAgent ?? null;
        export const requestAdapter = typeof globalThis.navigator?.gpu?.requestAdapter;
    "#,
    );
    let h = rt.load_module(&m).unwrap();
    let nav: String = rt.get_value(Some(&h), "navType").unwrap();
    let gpu: String = rt.get_value(Some(&h), "gpuType").unwrap();
    let ua: serde_json::Value = rt.get_value(Some(&h), "ua").unwrap();
    let req: String = rt.get_value(Some(&h), "requestAdapter").unwrap();
    println!("navigator: {nav}\nnavigator.gpu: {gpu}\nuserAgent: {ua}\nrequestAdapter: {req}");
}
