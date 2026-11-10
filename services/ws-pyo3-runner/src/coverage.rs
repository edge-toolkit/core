//! coverage.py measurement of the user module, for the repo's coverage workflow.
//!
//! Compiled in only under the `coverage` feature, and started only when `ET_TEST_COVERAGE` is `true`. The data lands
//! in `target/pycov/` as one `.coverage.<suffix>` file per process, where the combined Python report's `coverage
//! combine` already gathers the pytest and Pyodide data, so every module under `modules/pyo3/` that a measured runner
//! loads joins that report without being named anywhere.
//!
//! Two things a plain `Coverage.start()` would miss are handled here. The hooks run on the runner's own dispatch
//! thread, which `threading` never started, so it never receives the tracer coverage.py gives every thread that
//! `threading` starts; [`measured`] installs it before each hook. And the embedded interpreter is never finalized, so
//! nothing saves the data for us: the runner calls [`finish`] once its teardown has run the module's last hook, and a
//! timer saves meanwhile for a runner that is killed before it gets there. Neither save sits between a hook and the
//! reply it returns, which a slow save would otherwise hold back.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pyo3::prelude::*;
use pyo3::types::PyDict;
use tracing::error;

use crate::python::{PythonError, prepend_sys_path};

/// How often the background save runs, which bounds what a runner killed before [`finish`] loses.
const SAVE_INTERVAL: Duration = Duration::from_secs(1);

/// The `coverage.Coverage` measuring this process, once [`start`] has made it.
static COVERAGE: OnceLock<Py<PyAny>> = OnceLock::new();

/// A failure to start measuring the user module.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CoverageError {
    /// The data directory could not be created, or the thread that saves into it could not be spawned.
    #[error("setting up coverage measurement: {0}")]
    Io(#[from] std::io::Error),
    /// coverage.py could not be imported, configured or started.
    #[error(transparent)]
    Python(#[from] PythonError),
}

/// Start measuring every file under `modules/pyo3/`, before the user module is imported.
///
/// `source` is what keeps the tracer off everything else a module imports -- torch's import alone runs far more
/// Python than every module here put together. `config/coverage.toml` supplies the branch setting, which has to
/// match the pytest data this is combined with.
///
/// coverage.py itself is imported from `python_path`, the `sys.path` prefix the module resolves against, so it is
/// put in place first; mise's pipx site-packages are where the coverage env installs it. A module fetched from the
/// hub is compiled under its bare file name, which coverage.py resolves against `sys.path`; its directory has to be
/// on `PYO3_PYTHONPATH` for its lines to be attributed to it.
pub fn start(python_path: &[PathBuf]) -> Result<(), CoverageError> {
    let root = edge_toolkit::config::get_project_root();
    let data_dir = root.join("target/pycov");
    fs_err::create_dir_all(&data_dir)?;
    let config_file = root.join("config/coverage.toml").to_string_lossy().into_owned();
    let data_file = data_dir.join(".coverage").to_string_lossy().into_owned();
    let source = root.join("modules/pyo3").to_string_lossy().into_owned();
    // A suffix of our own rather than coverage.py's `True`. Given `True`, its first save renames the file after a hash
    // of the data saved so far, which is still empty in every process at that point, and `coverage combine` then
    // skips all but one of the files as duplicates. The time keeps a recycled pid from reusing a file.
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let suffix = format!("pyo3-runner.pid{}.{started}", std::process::id());
    Python::attach(|py| -> Result<(), PythonError> {
        prepend_sys_path(py, python_path)?;
        let options = PyDict::new(py);
        options.set_item("config_file", config_file)?;
        options.set_item("data_file", data_file)?;
        options.set_item("data_suffix", suffix)?;
        options.set_item("source", [source])?;
        let coverage = py.import("coverage")?.getattr("Coverage")?.call((), Some(&options))?;
        drop(coverage.call_method0("start")?);
        if COVERAGE.set(coverage.unbind()).is_err() {
            return Err(PythonError::Py("coverage measurement was already started".to_owned()));
        }
        Ok(())
    })?;
    let _saver = std::thread::Builder::new()
        .name("pycov-save".to_owned())
        .spawn(save_periodically)?;
    Ok(())
}

/// Wrap `hook` to run under coverage.py's tracer on the calling thread.
///
/// pyo3 creates a thread state for a thread Python did not start only for the length of one `Python::attach`, and
/// the tracer lives on that state, so it is installed for every hook rather than once for the thread. A no-op until
/// [`start`] has run. Failing to install it is an error rather than a log line: measurement that quietly stopped
/// reads as untested code in the report, not as a broken measurement.
pub fn measured<T, F>(hook: F) -> impl FnOnce(Python<'_>) -> Result<T, PythonError>
where
    F: FnOnce(Python<'_>) -> Result<T, PythonError>,
{
    move |py| {
        trace_current_thread(py)?;
        hook(py)
    }
}

/// Save the last of what was measured, once the module has run its last hook.
///
/// A no-op until [`start`] has run.
pub fn finish() -> Result<(), PythonError> {
    Python::attach(save)
}

/// Give the calling thread the tracer coverage.py gives every thread that `threading` starts.
fn trace_current_thread(py: Python<'_>) -> Result<(), PythonError> {
    if COVERAGE.get().is_none() {
        return Ok(());
    }
    let tracer = py.import("threading")?.call_method0("gettrace")?;
    drop(py.import("sys")?.call_method1("settrace", (tracer,))?);
    Ok(())
}

/// Write out what has been measured so far.
///
/// A no-op until [`start`] has run, so a coverage build run without `ET_TEST_COVERAGE` writes nothing.
fn save(py: Python<'_>) -> Result<(), PythonError> {
    if let Some(coverage) = COVERAGE.get() {
        drop(coverage.bind(py).call_method0("save")?);
    }
    Ok(())
}

/// Save every [`SAVE_INTERVAL`] until a save fails, for a runner that is killed before [`finish`].
fn save_periodically() {
    loop {
        std::thread::sleep(SAVE_INTERVAL);
        if let Err(err) = Python::attach(save) {
            error!("saving coverage failed, so nothing more is saved: {err}");
            return;
        }
    }
}
