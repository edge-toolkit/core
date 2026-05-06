/// Bisect the data1 stack overflow: load the module without polyfills/run.
///
/// Run with the local ws-server already up on :8080 (mise run ws-server):
///   cargo test -p et-ws-worker --test data1_load -- --ignored --nocapture
use et_ws_worker::{apply_browser_polyfills, create_runtime};
use rustyscript::deno_core::error::ModuleLoaderError;
use rustyscript::deno_core::{ModuleSource, ModuleSpecifier};
use rustyscript::module_loader::ImportProvider;
use rustyscript::{Module, Runtime, RuntimeOptions};
use std::time::Duration;

/// Identity import provider — touches every fetched module but rewrites nothing.
struct PassThroughProvider;
impl ImportProvider for PassThroughProvider {
    fn post_process(
        &mut self,
        _specifier: &ModuleSpecifier,
        source: ModuleSource,
    ) -> Result<ModuleSource, ModuleLoaderError> {
        Ok(source)
    }
}

const HTTP_BASE: &str = "http://localhost:8080";
const ENTRY_URL: &str = "http://localhost:8080/modules/et-ws-data1/et_ws_data1.js";

#[test]
#[ignore]
fn create_runtime_only() {
    let _runtime = create_runtime(HTTP_BASE).unwrap();
    println!("create_runtime ok");
}

#[test]
#[ignore]
fn create_runtime_and_polyfills() {
    let mut runtime = create_runtime(HTTP_BASE).unwrap();
    apply_browser_polyfills(&mut runtime, HTTP_BASE).unwrap();
    println!("polyfills ok");
}

#[test]
#[ignore]
fn load_module_without_polyfills() {
    let mut runtime = create_runtime(HTTP_BASE).unwrap();
    let stub = Module::new("entry.js", format!(r#"export {{ default, run }} from {ENTRY_URL:?};"#));
    let _handle = runtime.load_module(&stub).unwrap();
    println!("load_module ok");
}

#[test]
#[ignore]
fn load_module_default_runtime() {
    // No custom import_provider — does the bare rustyscript Runtime crash too?
    let mut runtime = Runtime::new(RuntimeOptions {
        timeout: Duration::from_secs(60),
        ..Default::default()
    })
    .unwrap();
    let stub = Module::new("entry.js", format!(r#"export {{ default, run }} from {ENTRY_URL:?};"#));
    let _handle = runtime.load_module(&stub).unwrap();
    println!("load_module ok");
}

#[test]
#[ignore]
fn load_with_import_provider_only() {
    // Custom import_provider but no WebOptions.base_url — isolate which one crashes.
    use rustyscript::ExtensionOptions;
    let mut runtime = Runtime::new(RuntimeOptions {
        timeout: Duration::from_secs(60),
        import_provider: Some(Box::new(et_ws_worker::RelativeUrlFixup {
            http_base: HTTP_BASE.to_string(),
        })),
        extension_options: ExtensionOptions::default(),
        ..Default::default()
    })
    .unwrap();
    let stub = Module::new("entry.js", format!(r#"export {{ default, run }} from {ENTRY_URL:?};"#));
    let _handle = runtime.load_module(&stub).unwrap();
    println!("load_module ok");
}

#[test]
#[ignore]
fn load_with_passthrough_provider() {
    // Identity provider — does merely registering an ImportProvider trigger the crash,
    // or is it the patching the source with var-shadowing that does it?
    let mut runtime = Runtime::new(RuntimeOptions {
        timeout: Duration::from_secs(60),
        import_provider: Some(Box::new(PassThroughProvider)),
        ..Default::default()
    })
    .unwrap();
    let stub = Module::new("entry.js", format!(r#"export {{ default, run }} from {ENTRY_URL:?};"#));
    let _handle = runtime.load_module(&stub).unwrap();
    println!("load_module ok");
}

#[test]
#[ignore]
fn load_trivial_remote_with_provider() {
    // Tiny remote JS + ImportProvider — does post_process by itself crash, or only on data1.js?
    let mut runtime = Runtime::new(RuntimeOptions {
        timeout: Duration::from_secs(60),
        import_provider: Some(Box::new(PassThroughProvider)),
        ..Default::default()
    })
    .unwrap();
    // Use a tiny file that the server happens to serve. package.json is JSON — try the .d.ts.
    // Fall back to a data: URL via inline import if needed.
    let stub = Module::new(
        "entry.js",
        r#"export const x = 1;"#.to_string(),
    );
    let _handle = runtime.load_module(&stub).unwrap();
    println!("inline ok");
}

#[test]
#[ignore]
fn load_trivial_remote_module() {
    // Load any tiny remote JS file: is the crash module-specific or any HTTP import?
    let mut runtime = Runtime::new(RuntimeOptions {
        timeout: Duration::from_secs(60),
        ..Default::default()
    })
    .unwrap();
    let trivial = "http://localhost:8080/modules/et-ws-data1/package.json";
    let stub = Module::new(
        "entry.js",
        format!(r#"const r = await fetch({trivial:?}); console.log("got", await r.text());"#),
    );
    let _handle = runtime.load_module(&stub).unwrap();
    println!("trivial load ok");
}

#[test]
#[ignore]
fn load_module_with_polyfills() {
    let mut runtime = create_runtime(HTTP_BASE).unwrap();
    apply_browser_polyfills(&mut runtime, HTTP_BASE).unwrap();
    let stub = Module::new("entry.js", format!(r#"export {{ default, run }} from {ENTRY_URL:?};"#));
    let _handle = runtime.load_module(&stub).unwrap();
    println!("load_module ok");
}
