//! Python module loader + dispatcher.
//!
//! The runner imports one user module by name (`PYO3_AGENT_MODULE`). The
//! module declares zero or more of these functions:
//!
//!     def init() -> object: ...
//!     def set_agent_id(state, agent_id: str) -> None: ...
//!     def handle_text(state, text: str) -> str | None: ...
//!     def handle_binary(state, frame: bytes) -> bytes | None: ...
//!     def shutdown(state) -> None: ...
//!
//! Each is optional — `Dispatcher` looks up callables by name with `hasattr`
//! and silently no-ops the ones missing. That keeps the contract small for
//! the simple case (e.g. binary-only echo) without forcing boilerplate.

use std::path::PathBuf;

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyList, PyModule, PyString};

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

/// Owns the imported user module and its (optional) state object across
/// the lifetime of the agent. Held by the WebSocket loop and called per
/// frame.
pub struct Dispatcher {
    module: Py<PyModule>,
    /// `init()`'s return value. `Py<PyAny>::None` when the module didn't
    /// expose `init`. Stored once so we don't have to round-trip through
    /// the GIL on every frame just to keep ownership.
    state: Py<PyAny>,
}

impl Dispatcher {
    /// Import the user module and run its `init()` hook.
    ///
    /// `python_path_extras` is prepended to `sys.path` so callers can point
    /// at the directory containing their module without installing it.
    pub fn import(module_name: &str, python_path_extras: &[PathBuf]) -> Result<Self, PythonError> {
        Python::attach(|py| -> Result<Self, PythonError> {
            if !python_path_extras.is_empty() {
                let sys = py.import("sys")?;
                let sys_path = sys.getattr("path")?.cast_into::<PyList>()?;
                for extra in python_path_extras {
                    let entry = PyString::new(py, &extra.to_string_lossy());
                    sys_path.insert(0, entry)?;
                }
            }

            let module = py.import(module_name)?;
            let state = if module.hasattr("init")? {
                module.call_method0("init")?.unbind()
            } else {
                py.None()
            };

            Ok(Self {
                module: module.unbind(),
                state,
            })
        })
    }

    /// Forward the assigned `agent_id` to the user module's optional
    /// `set_agent_id(state, agent_id)` hook.
    pub fn set_agent_id(&self, agent_id: &str) -> Result<(), PythonError> {
        Python::attach(|py| -> Result<(), PythonError> {
            let module = self.module.bind(py);
            if !module.hasattr("set_agent_id")? {
                return Ok(());
            }
            let state = self.state.bind(py);
            module.call_method1("set_agent_id", (state, agent_id))?;
            Ok(())
        })
    }

    /// Dispatch a text frame to `handle_text` if defined. Returns the
    /// module's reply (a `str`) as bytes-via-UTF-8, or `None` for no reply.
    pub fn handle_text(&self, text: &str) -> Result<Option<String>, PythonError> {
        Python::attach(|py| -> Result<Option<String>, PythonError> {
            let module = self.module.bind(py);
            if !module.hasattr("handle_text")? {
                return Ok(None);
            }
            let state = self.state.bind(py);
            let result = module.call_method1("handle_text", (state, text))?;
            if result.is_none() {
                return Ok(None);
            }
            // Accept both str (returned verbatim) and bytes (decoded as
            // utf-8) so modules don't get tripped up by Python's
            // bytes/str distinction.
            if let Ok(s) = result.extract::<String>() {
                return Ok(Some(s));
            }
            if let Ok(b) = result.extract::<Vec<u8>>() {
                return Ok(Some(String::from_utf8_lossy(&b).into_owned()));
            }
            Err(PythonError::Py(
                "handle_text must return str, bytes, or None".to_string(),
            ))
        })
    }

    /// Dispatch a binary frame to `handle_binary` if defined.
    pub fn handle_binary(&self, frame: &[u8]) -> Result<Option<Vec<u8>>, PythonError> {
        Python::attach(|py| -> Result<Option<Vec<u8>>, PythonError> {
            let module = self.module.bind(py);
            if !module.hasattr("handle_binary")? {
                return Ok(None);
            }
            let state = self.state.bind(py);
            let frame_obj = PyBytes::new(py, frame);
            let result = module.call_method1("handle_binary", (state, frame_obj))?;
            if result.is_none() {
                return Ok(None);
            }
            // Accept bytes, bytearray, memoryview-backed buffers via Vec<u8>
            // extraction. Also accept str — encode as utf-8 — so a Python
            // module computing a JSON response can return it without
            // .encode() boilerplate.
            if let Ok(b) = result.extract::<Vec<u8>>() {
                return Ok(Some(b));
            }
            if let Ok(s) = result.extract::<String>() {
                return Ok(Some(s.into_bytes()));
            }
            Err(PythonError::Py(
                "handle_binary must return bytes, str, or None".to_string(),
            ))
        })
    }

    /// Best-effort `shutdown(state)` call. Errors are swallowed by the
    /// caller so a misbehaving cleanup hook doesn't mask the connection
    /// closure that triggered it.
    pub fn shutdown(&self) -> Result<(), PythonError> {
        Python::attach(|py| -> Result<(), PythonError> {
            let module = self.module.bind(py);
            if !module.hasattr("shutdown")? {
                return Ok(());
            }
            let state = self.state.bind(py);
            module.call_method1("shutdown", (state,))?;
            Ok(())
        })
    }
}
