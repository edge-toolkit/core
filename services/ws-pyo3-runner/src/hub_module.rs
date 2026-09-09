//! Fetching a user module from the hub, for names that do not resolve locally.
//!
//! The runner's original and still-primary way of finding its module is a plain `sys.path` import, which is why
//! this is a fallback rather than the rule: a `RUNNER_MODULE` that `PYO3_PYTHONPATH` (or mise's site-packages)
//! already makes importable never reaches this module, and behaves exactly as it did before hub fetching
//! existed. What it adds is the shape the other two runners already have -- name a module the hub publishes and
//! the runner goes and gets it -- so a deployment can point a pyo3 runner at a module without also having to
//! put that module on the runner's filesystem.
//!
//! The payload is the file named by the module's `package.json` `main`, which for a pyo3 module is a single
//! `.py`. Its stem becomes the imported module name, exactly as `main` names the `.wasm` a WASI component is
//! instantiated from. One file is the whole contract: a module needing more than that is a wheel's job, and
//! nothing here would have to change to load one from `sys.path`.

use std::path::PathBuf;

use et_ws_runner_common::{collect_byte_stream, derive_http_base, fetch_main_field};
use tracing::Instrument as _;

use crate::error::RunnerError;
use crate::python::module_is_importable;

/// A user module fetched from the hub, ready to be compiled.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct HubModule {
    /// Name the source is compiled under: the stem of the hub's `main` file.
    pub import_name: String,
    /// The module's Python source.
    pub source: String,
}

/// Fetch `module_name` from the hub, unless it is already importable locally.
///
/// Returns `None` when the local import will work, which is the caller's signal to load it the way it always
/// has. The probe runs first so that no hub request is made at all in that case -- a runner given a local
/// module keeps working against a hub that has never heard of it.
pub async fn fetch_if_absent(
    module_name: &str,
    python_path: &[PathBuf],
    ws_url: &str,
) -> Result<Option<HubModule>, RunnerError> {
    if module_is_importable(module_name, python_path)? {
        tracing::debug!(module = module_name, "module found on sys.path; not fetching");
        return Ok(None);
    }
    let http_base = derive_http_base(ws_url)?;
    let rest = et_rest_client::Client::new(&http_base);
    // Retries while the hub is still scanning its module paths, so a runner started alongside the hub is not
    // lost to a 404 for a module that is about to appear.
    let main = fetch_main_field(&rest, module_name).await?;
    tracing::info!(module = module_name, %main, "fetching Python module from the hub");
    let bytes = fetch_module_file(&rest, module_name, &main).await?;
    let source = match String::from_utf8(bytes) {
        Ok(source) => source,
        Err(_not_utf8) => {
            return Err(RunnerError::ModuleNotUtf8 {
                module: module_name.to_string(),
                file: main,
            });
        }
    };
    Ok(Some(HubModule {
        import_name: import_name_for(&main),
        source,
    }))
}

/// Download one of a module's published files.
///
/// Returns `BootstrapError` rather than the caller's error type so the REST failure converts through the `From`
/// impl the shared runner crate already carries, and `?` does the work at both levels.
async fn fetch_module_file(
    rest: &et_rest_client::Client,
    module_name: &str,
    main: &str,
) -> Result<Vec<u8>, et_ws_runner_common::BootstrapError> {
    let response = rest
        .get_module_file(module_name, main)
        .instrument(tracing::info_span!("fetch_module", module = module_name, file = %main))
        .await?;
    collect_byte_stream(response.into_inner()).await
}

/// Derive the name a fetched file is compiled under from the hub's `main` entry.
///
/// The stem rather than `module_name`, because the hub publishes a module under its package name -- which is
/// `et-ws-pyo3-math1`, not a legal Python identifier. The file it serves is named for the module it defines,
/// so that is the name the source has to be compiled under for `__name__` and tracebacks to read right.
fn import_name_for(main: &str) -> String {
    main.rsplit('/')
        .next()
        .unwrap_or(main)
        .strip_suffix(".py")
        .unwrap_or(main)
        .to_string()
}
