//! PyO3 driver for the split-learning server-side model.
//!
//! Embeds CPython via PyO3's `auto-initialize` feature, imports the demo's
//! `split_learning` package and our adapter module (`server_impl`), and
//! exposes a thin `ModelHandle` the WebSocket loop calls into.
//!
//! The adapter module is loaded from `python/server_impl.py` at build time
//! via `include_str!`, then registered into `sys.modules` so it can `import
//! split_learning.*` and find the demo's installed package on `PYTHONPATH`.
//!
//! All Python state (the model, optimizer, Fabric handle) lives behind one
//! `Py<PyAny>` representing the dict that `server_impl.init_state` returns.
//! The Rust side never inspects it.

use std::ffi::CString;
use std::path::{Path, PathBuf};

use pyo3::IntoPyObjectExt;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList, PyModule, PyString};

const SERVER_IMPL_SOURCE: &str = include_str!("../python/server_impl.py");

#[derive(Debug, thiserror::Error)]
pub enum PythonError {
    #[error("python: {0}")]
    Py(String),
}

impl From<PyErr> for PythonError {
    fn from(err: PyErr) -> Self {
        Self::Py(format!("{err}"))
    }
}

impl<'a, 'py> From<pyo3::CastError<'a, 'py>> for PythonError {
    fn from(err: pyo3::CastError<'a, 'py>) -> Self {
        Self::Py(format!("cast: {err}"))
    }
}

impl<'py> From<pyo3::CastIntoError<'py>> for PythonError {
    fn from(err: pyo3::CastIntoError<'py>) -> Self {
        Self::Py(format!("cast: {err}"))
    }
}

/// Owns the PyO3 reference to the `server_impl` module and to the state dict
/// `init_state` produced. Both are `Py<...>` (GIL-independent) so we can hold
/// the handle across async awaits without locking the interpreter the whole
/// time.
pub struct ModelHandle {
    module: Py<PyModule>,
    state: Py<PyAny>,
    onnx_path: Option<PathBuf>,
}

impl ModelHandle {
    /// Build the server-side model. If `onnx_path` exists it's loaded as the
    /// starting weights; otherwise the model starts from random init. The
    /// same `onnx_path` is used for `export_weights` on shutdown.
    pub fn new(
        onnx_path: Option<PathBuf>,
        learning_rate: f64,
        accelerator: &str,
        python_path_extras: &[PathBuf],
    ) -> Result<Self, PythonError> {
        Python::attach(|py| -> Result<Self, PythonError> {
            // Prepend each extra path to sys.path so `import split_learning`
            // finds the demo's package without requiring a system-wide
            // install. The split-learning-demo lays out its src/ tree under
            // packages/split-learning-demo/src.
            if !python_path_extras.is_empty() {
                let sys = py.import("sys")?;
                let sys_path = sys.getattr("path")?.cast_into::<PyList>()?;
                for extra in python_path_extras {
                    let entry = PyString::new(py, &extra.to_string_lossy());
                    sys_path.insert(0, entry)?;
                }
            }

            // PyModule::from_code took &str in 0.22 but now wants &CStr.
            // The source is a `&'static str` from `include_str!`, so we
            // build the trailing NUL once at startup — no per-call cost.
            let source = CString::new(SERVER_IMPL_SOURCE)
                .map_err(|e| PythonError::Py(format!("server_impl.py contains a NUL byte: {e}")))?;
            let filename = c"server_impl.py";
            let module_name = c"server_impl";
            let module = PyModule::from_code(py, source.as_c_str(), filename, module_name)?;

            let onnx_arg = match &onnx_path {
                Some(p) => p.to_string_lossy().into_py_any(py)?,
                None => py.None(),
            };
            let kwargs = PyDict::new(py);
            kwargs.set_item("learning_rate", learning_rate)?;
            kwargs.set_item("accelerator", accelerator)?;
            let state = module.call_method("init_state", (onnx_arg,), Some(&kwargs))?;

            Ok(Self {
                module: module.into(),
                state: state.into(),
                onnx_path,
            })
        })
    }

    /// Run one training step. Returns `(grad_bytes, grad_shape, loss)`.
    pub fn process_activations_and_labels(
        &self,
        activation_bytes: &[u8],
        label_bytes: &[u8],
        tensor_shape: &[i64],
    ) -> Result<(Vec<u8>, Vec<i64>, f64), PythonError> {
        Python::attach(|py| -> Result<_, PythonError> {
            let module = self.module.bind(py);
            let state = self.state.bind(py);
            let activations = PyBytes::new(py, activation_bytes);
            let labels = PyBytes::new(py, label_bytes);
            let shape = PyList::new(py, tensor_shape)?;
            let result = module.call_method1(
                "process_activations_and_labels",
                (state, activations, labels, shape),
            )?;
            let dict = result.cast::<PyDict>()?;
            let tensor = dict
                .get_item("tensor")?
                .ok_or_else(|| PythonError::Py("missing tensor".into()))?
                .cast::<PyBytes>()?
                .as_bytes()
                .to_vec();
            let shape_value = dict
                .get_item("tensor_shape")?
                .ok_or_else(|| PythonError::Py("missing tensor_shape".into()))?;
            let shape_out = extract_shape(&shape_value)?;
            let loss: f64 = dict
                .get_item("loss")?
                .ok_or_else(|| PythonError::Py("missing loss".into()))?
                .extract()?;
            Ok((tensor, shape_out, loss))
        })
    }

    /// Run one inference step. Returns `(logits_bytes, logits_shape)`.
    pub fn process_activations(
        &self,
        activation_bytes: &[u8],
        tensor_shape: &[i64],
    ) -> Result<(Vec<u8>, Vec<i64>), PythonError> {
        Python::attach(|py| -> Result<_, PythonError> {
            let module = self.module.bind(py);
            let state = self.state.bind(py);
            let activations = PyBytes::new(py, activation_bytes);
            let shape = PyList::new(py, tensor_shape)?;
            let result = module.call_method1("process_activations", (state, activations, shape))?;
            let dict = result.cast::<PyDict>()?;
            let tensor = dict
                .get_item("tensor")?
                .ok_or_else(|| PythonError::Py("missing tensor".into()))?
                .cast::<PyBytes>()?
                .as_bytes()
                .to_vec();
            let shape_value = dict
                .get_item("tensor_shape")?
                .ok_or_else(|| PythonError::Py("missing tensor_shape".into()))?;
            let shape_out = extract_shape(&shape_value)?;
            Ok((tensor, shape_out))
        })
    }

    /// Persist trained weights to the configured ONNX path, if any.
    /// No-op when the model was never trained this session (mirrors
    /// `server.py`'s `trained_this_session` gate).
    pub fn export_weights(&self) -> Result<(), PythonError> {
        let Some(path) = self.onnx_path.as_deref() else {
            return Ok(());
        };
        Python::attach(|py| -> Result<(), PythonError> {
            let module = self.module.bind(py);
            let state = self.state.bind(py);
            module.call_method1("export_state", (state, path.to_string_lossy().into_owned()))?;
            Ok(())
        })
    }
}

/// Convert a `PyAny` representing a list/tuple of ints into a Rust `Vec<i64>`.
/// torch.Size is iterable like a tuple, so this covers both shapes the Python
/// helper might return.
fn extract_shape(value: &Bound<'_, PyAny>) -> Result<Vec<i64>, PythonError> {
    let dims: Vec<i64> = value
        .extract()
        .map_err(|e| PythonError::Py(format!("tensor_shape was not extractable as Vec<i64>: {e}")))?;
    Ok(dims)
}

/// Locate the split-learning-demo package directory.
///
/// `SPLIT_LEARNING_DEMO_SRC` overrides this; otherwise we look one level up
/// from the workspace at `../split-learning-demo/packages/split-learning-demo/src`,
/// which is where the user keeps the upstream demo checkout.
pub fn default_python_path_extras() -> Vec<PathBuf> {
    if let Ok(custom) = std::env::var("SPLIT_LEARNING_DEMO_SRC") {
        return vec![PathBuf::from(custom)];
    }
    // CARGO_MANIFEST_DIR points at this crate at compile time of the binary.
    // We walk up to the workspace root + sibling `split-learning-demo`.
    let manifest_dir: &Path = Path::new(env!("CARGO_MANIFEST_DIR"));
    let candidate = manifest_dir
        .ancestors()
        .nth(3) // crate -> ws-modules -> services -> repo root
        .map(|root| root.join("split-learning-demo/packages/split-learning-demo/src"));
    candidate.into_iter().collect()
}
