//! Python module loader + dispatcher.
//!
//! The runner imports one user module (`PYO3_AGENT_MODULE`) and invokes
//! module-level functions on it. The user module owns its state via
//! module-level globals; the runner never sees that state.
//!
//! On startup the runner hands the module a `WsSender` instance (this
//! crate's pyclass). The module can call `send.binary(...)` /
//! `send.text(...)` any number of times — during a handler call, from
//! a follow-up handler, or from a background thread — to push frames
//! out. Handlers return nothing; they don't have a "reply" channel,
//! they have a sender they can use whenever.
//!
//! Contract (all hooks optional):
//!
//!     _send = None
//!     def init(send): global _send; _send = send   # called once at startup
//!     def on_connect(agent_id: str) -> None: ...
//!     def on_text_frame(text: str) -> str | bytes | None: ...
//!     def on_binary_frame(frame: bytes) -> bytes | str | None: ...
//!     def on_shutdown() -> None: ...
//!
//! `on_text_frame` / `on_binary_frame` may return a single reply for the
//! simple case (echo, request/response). A handler that wants to emit
//! multiple frames, or emit nothing now and a frame later from a thread,
//! ignores the return value and uses the `WsSender` it stashed at
//! `init()`. The two styles compose: any sends made during a handler go
//! out *before* the returned reply, because both push onto the same
//! outbound queue and the queue is drained in order.

use std::path::PathBuf;

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyList, PyModule, PyString};
use tokio::sync::mpsc;

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

/// One frame queued on the agent's outbound channel. The WS loop in
/// `agent.rs` drains this and writes to the socket.
#[derive(Debug, Clone)]
pub enum OutboundFrame {
    Text(String),
    Binary(Vec<u8>),
}

/// Python-facing handle. Bound to a `tokio::sync::mpsc::UnboundedSender`
/// so Python's `.text()` / `.binary()` calls fire and forget — no GIL
/// hand-back across an await, no head-of-line blocking on the socket.
/// The handle is `Clone` so Python can stash multiple references if it
/// wants (e.g. across background threads).
#[pyclass(name = "WsSender")]
pub struct WsSender {
    tx: mpsc::UnboundedSender<OutboundFrame>,
}

#[pymethods]
impl WsSender {
    /// Queue a text frame for the agent's outbound socket.
    fn text(&self, text: String) -> PyResult<()> {
        self.tx
            .send(OutboundFrame::Text(text))
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!("ws send: {e}")))
    }

    /// Queue a binary frame for the agent's outbound socket. `frame`
    /// accepts any Python buffer protocol object (bytes / bytearray /
    /// memoryview) — PyO3's `Vec<u8>` extraction handles the conversion.
    fn binary(&self, frame: Vec<u8>) -> PyResult<()> {
        self.tx
            .send(OutboundFrame::Binary(frame))
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!("ws send: {e}")))
    }

    fn __repr__(&self) -> String {
        "<WsSender>".to_string()
    }
}

impl WsSender {
    pub fn new(tx: mpsc::UnboundedSender<OutboundFrame>) -> Self {
        Self { tx }
    }
}

/// Holds the imported user module across the lifetime of the agent loop.
pub struct Dispatcher {
    module: Py<PyModule>,
}

impl Dispatcher {
    /// Import the user module, prepend `python_path_extras` to `sys.path`,
    /// and call its optional `init(send)` hook with `sender`.
    pub fn import(
        module_name: &str,
        python_path_extras: &[PathBuf],
        sender: WsSender,
    ) -> Result<Self, PythonError> {
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
            if module.hasattr("init")? {
                let py_sender = Py::new(py, sender)?;
                module.call_method1("init", (py_sender,))?;
            }
            Ok(Self {
                module: module.unbind(),
            })
        })
    }

    /// Forward the assigned `agent_id` to the user's optional
    /// `on_connect(agent_id)` hook.
    pub fn on_connect(&self, agent_id: &str) -> Result<(), PythonError> {
        Python::attach(|py| -> Result<(), PythonError> {
            let module = self.module.bind(py);
            if !module.hasattr("on_connect")? {
                return Ok(());
            }
            module.call_method1("on_connect", (agent_id,))?;
            Ok(())
        })
    }

    /// Dispatch a text frame to `on_text_frame`. Returns the handler's
    /// optional reply (a `str`, or `bytes` decoded as utf-8). Outbound
    /// frames the handler queued via `WsSender` go out independently.
    pub fn on_text_frame(&self, text: &str) -> Result<Option<String>, PythonError> {
        Python::attach(|py| -> Result<Option<String>, PythonError> {
            let module = self.module.bind(py);
            if !module.hasattr("on_text_frame")? {
                return Ok(None);
            }
            let result = module.call_method1("on_text_frame", (text,))?;
            if result.is_none() {
                return Ok(None);
            }
            if let Ok(s) = result.extract::<String>() {
                return Ok(Some(s));
            }
            if let Ok(b) = result.extract::<Vec<u8>>() {
                return Ok(Some(String::from_utf8_lossy(&b).into_owned()));
            }
            Err(PythonError::Py(
                "on_text_frame must return str, bytes, or None".to_string(),
            ))
        })
    }

    /// Dispatch a binary frame to `on_binary_frame`. Returns the
    /// handler's optional reply (`bytes`, or `str` encoded as utf-8).
    pub fn on_binary_frame(&self, frame: &[u8]) -> Result<Option<Vec<u8>>, PythonError> {
        Python::attach(|py| -> Result<Option<Vec<u8>>, PythonError> {
            let module = self.module.bind(py);
            if !module.hasattr("on_binary_frame")? {
                return Ok(None);
            }
            let frame_obj = PyBytes::new(py, frame);
            let result = module.call_method1("on_binary_frame", (frame_obj,))?;
            if result.is_none() {
                return Ok(None);
            }
            if let Ok(b) = result.extract::<Vec<u8>>() {
                return Ok(Some(b));
            }
            if let Ok(s) = result.extract::<String>() {
                return Ok(Some(s.into_bytes()));
            }
            Err(PythonError::Py(
                "on_binary_frame must return bytes, str, or None".to_string(),
            ))
        })
    }

    /// Best-effort `on_shutdown()` call.
    pub fn on_shutdown(&self) -> Result<(), PythonError> {
        Python::attach(|py| -> Result<(), PythonError> {
            let module = self.module.bind(py);
            if !module.hasattr("on_shutdown")? {
                return Ok(());
            }
            module.call_method0("on_shutdown")?;
            Ok(())
        })
    }
}
