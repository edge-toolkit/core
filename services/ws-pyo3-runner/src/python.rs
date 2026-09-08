//! Python module loader + dispatcher.
//!
//! The runner imports one user module (`RUNNER_MODULE`) and invokes
//! module-level functions on it. The user module owns its state via
//! module-level globals; the runner never sees that state.
//!
//! On startup the runner hands the module two pyclass handles:
//!   * `WsSender` -- push outbound frames to the ws-server
//!   * `WsStorage` -- get/put files via the ws-server's `/storage` HTTP API
//!
//! Both are useful from anywhere in Python: during a handler, from a
//! later handler, or from a background thread the module spawns.
//! Handlers return either an optional reply (sent as one outbound
//! frame) or `None`.
//!
//! Contract (all hooks optional), as Python:
//!
//!     _send = None
//!     _storage = None
//!
//!     def init(send, storage):
//!         global _send, _storage
//!         _send, _storage = send, storage
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

use std::ffi::CString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyList, PyString};
use tokio::sync::mpsc;

// Names of the optional module-level hooks the runner calls. Each is referenced
// twice -- a `hasattr` guard and the call -- so it lives here as one source of
// truth: a typo in either spot would silently skip the hook (hasattr returns
// false, the hook never fires, no error). See the module-level contract above.
const HOOK_INIT: &str = "init";
const HOOK_ON_CONNECT: &str = "on_connect";
const HOOK_ON_TEXT_FRAME: &str = "on_text_frame";
const HOOK_ON_BINARY_FRAME: &str = "on_binary_frame";
const HOOK_ON_SHUTDOWN: &str = "on_shutdown";

/// Every hook, for the load-time sanity check.
///
/// A module that defines none of them can never be invoked, so importing it is
/// almost certainly a mistake.
const HOOKS: [&str; 5] = [
    HOOK_INIT,
    HOOK_ON_CONNECT,
    HOOK_ON_TEXT_FRAME,
    HOOK_ON_BINARY_FRAME,
    HOOK_ON_SHUTDOWN,
];

/// An error from importing or invoking the user's Python module.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PythonError {
    #[error("python: {0}")]
    Py(String),
}

impl From<PyErr> for PythonError {
    fn from(err: PyErr) -> Self {
        Self::Py(format!("{err}"))
    }
}

impl<'cast, 'py> From<pyo3::CastError<'cast, 'py>> for PythonError {
    fn from(err: pyo3::CastError<'cast, 'py>) -> Self {
        Self::Py(format!("cast: {err}"))
    }
}

impl<'py> From<pyo3::CastIntoError<'py>> for PythonError {
    fn from(err: pyo3::CastIntoError<'py>) -> Self {
        Self::Py(format!("cast: {err}"))
    }
}

/// A storage GET/PUT that the worker in `agent.rs` failed to complete.
///
/// Typed so the failing operation (and the agent/key it targeted) survive
/// back to the Python caller, where `From<StorageError>` turns it into a
/// `RuntimeError`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StorageError {
    #[error("GET {agent_id}/{key}: {message}")]
    Get {
        agent_id: String,
        key: String,
        message: String,
    },
    #[error("PUT {agent_id}/{key}: {message}")]
    Put {
        agent_id: String,
        key: String,
        message: String,
    },
}

impl StorageError {
    /// Build a `Get` failure.
    ///
    /// Borrows the identifiers so the worker's error arms don't fight the borrow
    /// checker over the moved op fields.
    #[must_use]
    pub fn get(agent_id: &str, key: &str, message: String) -> Self {
        Self::Get {
            agent_id: agent_id.to_owned(),
            key: key.to_owned(),
            message,
        }
    }

    /// Build a `Put` failure; see [`StorageError::get`].
    #[must_use]
    pub fn put(agent_id: &str, key: &str, message: String) -> Self {
        Self::Put {
            agent_id: agent_id.to_owned(),
            key: key.to_owned(),
            message,
        }
    }
}

impl From<StorageError> for PyErr {
    fn from(err: StorageError) -> Self {
        PyRuntimeError::new_err(err.to_string())
    }
}

/// One frame queued on the agent's outbound channel.
///
/// The WS loop in `agent.rs` drains this and writes to the socket.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum OutboundFrame {
    Text(String),
    Binary(Vec<u8>),
}

/// Python-facing handle for queuing outbound frames.
///
/// Bound to a `tokio::sync::mpsc::UnboundedSender` so Python's `.text()` /
/// `.binary()` calls fire and forget -- no GIL hand-back across an await, no
/// head-of-line blocking on the socket. The handle is `Clone` so Python can
/// stash multiple references if it wants (e.g. across background threads).
#[pyclass(name = "WsSender")]
pub struct WsSender {
    tx: mpsc::UnboundedSender<OutboundFrame>,
}

#[pymethods]
impl WsSender {
    /// Queue a text frame for the agent's outbound socket.
    fn text(&self, text: String) -> PyResult<()> {
        match self.tx.send(OutboundFrame::Text(text)) {
            Ok(()) => Ok(()),
            Err(err) => Err(PyRuntimeError::new_err(format!("ws send: {err}"))),
        }
    }

    /// Queue a binary frame for the agent's outbound socket.
    ///
    /// `frame` accepts any Python buffer protocol object (bytes / bytearray /
    /// memoryview) -- `PyO3`'s `Vec<u8>` extraction handles the conversion.
    fn binary(&self, frame: Vec<u8>) -> PyResult<()> {
        match self.tx.send(OutboundFrame::Binary(frame)) {
            Ok(()) => Ok(()),
            Err(err) => Err(PyRuntimeError::new_err(format!("ws send: {err}"))),
        }
    }

    #[expect(
        clippy::unused_self,
        reason = "pyo3 #[pymethods] __repr__ takes &self by Python convention"
    )]
    fn __repr__(&self) -> String {
        "<WsSender>".to_string()
    }
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "pyo3 #[pymethods] expands to its own inherent impl; the Rust-only constructor lives here"
)]
impl WsSender {
    /// Construct a [`WsSender`] over the outbound-frame channel.
    #[must_use]
    pub const fn new(tx: mpsc::UnboundedSender<OutboundFrame>) -> Self {
        Self { tx }
    }
}

/// Shared cell holding the `agent_id` assigned by et-connect-ack.
///
/// The runner writes it from `agent.rs::register`; Python reads it via
/// `WsStorage.agent_id`. Pre-connect it's `None`, so writes that target
/// the agent's own namespace fail with a clear error.
pub type AgentIdSlot = Arc<Mutex<Option<String>>>;

/// One unit of work the storage worker task knows how to do.
///
/// Python's sync `WsStorage.get/put` build one of these, hand it off
/// via `op_tx`, and `blocking_recv()` on the embedded oneshot. The
/// worker (spawned by `agent.rs`) runs async `et-rest-client` calls
/// and sends results back.
#[derive(Debug)]
#[non_exhaustive]
pub enum StorageOp {
    Get {
        agent_id: String,
        key: String,
        reply: tokio::sync::oneshot::Sender<Result<Option<Vec<u8>>, StorageError>>,
    },
    Put {
        agent_id: String,
        key: String,
        data: Vec<u8>,
        reply: tokio::sync::oneshot::Sender<Result<(), StorageError>>,
    },
}

/// Python-facing handle to et-ws-server's `/storage` HTTP API.
///
/// `PUT /storage/<our-agent-id>/<key>` to persist, `GET /storage/<agent-id>/<key>`
/// to read (any agent's namespace is readable since the server static-serves
/// `/storage/`; writes only succeed for our own scope). Methods look
/// synchronous to Python -- internally they dispatch to a worker task on
/// the runtime and block on a oneshot reply.
#[pyclass(name = "WsStorage")]
pub struct WsStorage {
    agent_id: AgentIdSlot,
    op_tx: mpsc::UnboundedSender<StorageOp>,
}

#[pymethods]
impl WsStorage {
    /// Return our currently assigned `agent_id`, or `None` before `on_connect`.
    #[getter]
    fn agent_id(&self) -> Option<String> {
        self.agent_id.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// GET `/storage/{agent_id}/{key}`.
    ///
    /// Returns `None` for 404, raises on other HTTP failures. Reads work for any
    /// agent's namespace (et-storage-service static-serves the storage directory).
    fn get(&self, py: Python<'_>, agent_id: String, key: String) -> PyResult<Option<Vec<u8>>> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        if self
            .op_tx
            .send(StorageOp::Get {
                agent_id,
                key,
                reply: reply_tx,
            })
            .is_err()
        {
            return Err(PyRuntimeError::new_err("storage worker gone"));
        }
        // `detach` drops the GIL so other Python threads run while we park
        // here. We're always called from a non-runtime thread -- the dedicated
        // dispatch thread, or one the module spawned -- so a plain
        // `blocking_recv` is correct: the storage worker task resolves the
        // reply on the runtime's own threads while this thread waits.
        match py.detach(|| reply_rx.blocking_recv()) {
            Ok(result) => Ok(result?),
            Err(_) => Err(PyRuntimeError::new_err("storage reply dropped")),
        }
    }

    /// PUT to `/storage/<our-agent-id>/{key}`.
    ///
    /// Errors if `on_connect` hasn't fired yet (we don't know our `agent_id`) --
    /// call this from `on_connect` or later.
    fn put(&self, py: Python<'_>, key: String, data: Vec<u8>) -> PyResult<()> {
        let agent_id = self
            .agent_id
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or_else(|| {
                PyRuntimeError::new_err("WsStorage.put() called before on_connect -- agent_id not yet assigned")
            })?;
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        if self
            .op_tx
            .send(StorageOp::Put {
                agent_id,
                key,
                data,
                reply: reply_tx,
            })
            .is_err()
        {
            return Err(PyRuntimeError::new_err("storage worker gone"));
        }
        // See `get` for why a plain `blocking_recv` (not `block_in_place`) is
        // correct here: we never run on a tokio worker thread.
        match py.detach(|| reply_rx.blocking_recv()) {
            Ok(result) => {
                result?;
                Ok(())
            }
            Err(_) => Err(PyRuntimeError::new_err("storage reply dropped")),
        }
    }

    fn __repr__(&self) -> String {
        format!("<WsStorage agent_id={:?}>", self.agent_id())
    }
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "pyo3 #[pymethods] expands to its own inherent impl; the Rust-only constructor lives here"
)]
impl WsStorage {
    /// Construct a [`WsStorage`] over the agent-id slot and op channel.
    #[must_use]
    pub const fn new(agent_id: AgentIdSlot, op_tx: mpsc::UnboundedSender<StorageOp>) -> Self {
        Self { agent_id, op_tx }
    }
}

/// Convert to the NUL-terminated form `CPython`'s compiler takes, naming `what` if the value has an interior NUL.
///
/// Python source and module names reach the runner from the wire, so a NUL is reachable input rather than a
/// programmer error, and it has to come back as a load failure naming what was wrong.
fn nul_free(value: &str, what: &str) -> Result<CString, PythonError> {
    match CString::new(value) {
        Ok(c_string) => Ok(c_string),
        Err(_interior_nul) => Err(PythonError::Py(format!("{what} contains a NUL byte"))),
    }
}

/// Prepend `extras` to `sys.path`, skipping any entry already on it.
///
/// Called once per load attempt, and a load can be attempted twice -- [`module_is_importable`] probes before
/// [`Dispatcher::import`] runs -- so the same entries would otherwise pile up and `sys.path` would grow a
/// duplicate for every extra.
fn prepend_sys_path(py: Python<'_>, extras: &[PathBuf]) -> Result<(), PythonError> {
    if extras.is_empty() {
        return Ok(());
    }
    let sys = py.import("sys")?;
    let sys_path = sys.getattr("path")?.cast_into::<PyList>()?;
    for extra in extras {
        let entry = PyString::new(py, &extra.to_string_lossy());
        if !sys_path.contains(&entry)? {
            sys_path.insert(0, entry)?;
        }
    }
    Ok(())
}

/// Report whether `module_name` can be found on `sys.path` once `extras` are on it.
///
/// This is what decides between the two ways the runner gets its module: a name that resolves locally is
/// imported as it always has been, and only a name that does not is fetched from the hub. `find_spec` locates
/// the module without executing it, so the probe cannot half-run a module that the import below then runs again.
///
/// Any error from `find_spec` is reported as "not importable" rather than propagated. A missing parent package
/// raises rather than returning `None`, and a name the hub serves -- `et-ws-pyo3-math1`, say -- is not a legal
/// identifier at all; both mean the same thing here, and a name that is genuinely broken surfaces when the
/// import itself is attempted.
pub fn module_is_importable(module_name: &str, extras: &[PathBuf]) -> Result<bool, PythonError> {
    Python::attach(|py| -> Result<bool, PythonError> {
        prepend_sys_path(py, extras)?;
        let util = py.import("importlib.util")?;
        Ok(util
            .call_method1("find_spec", (module_name,))
            .is_ok_and(|spec| !spec.is_none()))
    })
}

/// Holds the imported user module across the lifetime of the agent loop.
pub struct Dispatcher {
    module: Py<PyModule>,
}

impl Dispatcher {
    /// Import the user module and run its optional `init(send, storage)` hook.
    ///
    /// `python_path_extras` are prepended to `sys.path` before the import so the
    /// module (and its dependencies) resolve.
    pub fn import(
        module_name: &str,
        python_path_extras: &[PathBuf],
        sender: WsSender,
        storage: WsStorage,
    ) -> Result<Self, PythonError> {
        Python::attach(|py| -> Result<Self, PythonError> {
            prepend_sys_path(py, python_path_extras)?;
            let module = py.import(module_name)?;
            Self::from_module(py, &module, module_name, sender, storage)
        })
    }

    /// Build the user module from `source` fetched over the wire, rather than importing it from `sys.path`.
    ///
    /// `module_name` is the name the module is compiled under, so tracebacks and `__name__` read as they would
    /// had it been imported normally. The module is not inserted into `sys.modules`: nothing else in the process
    /// imports it, and leaving it out keeps a fetched module from shadowing a same-named one on `sys.path`.
    ///
    /// `python_path_extras` still apply -- the source is compiled here, but whatever it imports resolves through
    /// the usual paths, so a fetched module's third-party dependencies come from mise's site-packages exactly as
    /// a local module's do.
    pub fn import_source(
        module_name: &str,
        source: &str,
        python_path_extras: &[PathBuf],
        sender: WsSender,
        storage: WsStorage,
    ) -> Result<Self, PythonError> {
        let code = nul_free(source, "module source")?;
        let file_name = nul_free(&format!("{module_name}.py"), "module file name")?;
        let name = nul_free(module_name, "module name")?;
        Python::attach(|py| -> Result<Self, PythonError> {
            prepend_sys_path(py, python_path_extras)?;
            let module = PyModule::from_code(py, &code, &file_name, &name)?;
            Self::from_module(py, &module, module_name, sender, storage)
        })
    }

    /// Check the hook contract and run `init`, for a module however it was loaded.
    fn from_module(
        py: Python<'_>,
        module: &Bound<'_, PyModule>,
        module_name: &str,
        sender: WsSender,
        storage: WsStorage,
    ) -> Result<Self, PythonError> {
        // Sanity check: a module that defines none of the hooks can never
        // be driven, so importing it is almost certainly a misconfiguration
        // (wrong RUNNER_MODULE, or a misspelt hook). Fail loudly at load
        // rather than connect and sit idle.
        let mut has_hook = false;
        for hook in HOOKS {
            if module.hasattr(hook)? {
                has_hook = true;
                break;
            }
        }
        if !has_hook {
            return Err(PythonError::Py(format!(
                "module `{module_name}` defines none of the runner hooks ({})",
                HOOKS.join(", ")
            )));
        }

        if module.hasattr(HOOK_INIT)? {
            let py_sender = Py::new(py, sender)?;
            let py_storage = Py::new(py, storage)?;
            drop(module.call_method1(HOOK_INIT, (py_sender, py_storage))?);
        }
        Ok(Self {
            module: module.clone().unbind(),
        })
    }

    /// Forward the assigned `agent_id` to the optional `on_connect` hook.
    pub fn on_connect(&self, agent_id: &str) -> Result<(), PythonError> {
        Python::attach(|py| -> Result<(), PythonError> {
            let module = self.module.bind(py);
            if !module.hasattr(HOOK_ON_CONNECT)? {
                return Ok(());
            }
            drop(module.call_method1(HOOK_ON_CONNECT, (agent_id,))?);
            Ok(())
        })
    }

    /// Dispatch a text frame to `on_text_frame`.
    ///
    /// Returns the handler's optional reply (a `str`, or `bytes` decoded as
    /// utf-8). Outbound frames the handler queued via `WsSender` go out
    /// independently.
    pub fn on_text_frame(&self, text: &str) -> Result<Option<String>, PythonError> {
        Python::attach(|py| -> Result<Option<String>, PythonError> {
            let module = self.module.bind(py);
            if !module.hasattr(HOOK_ON_TEXT_FRAME)? {
                return Ok(None);
            }
            let result = module.call_method1(HOOK_ON_TEXT_FRAME, (text,))?;
            if result.is_none() {
                return Ok(None);
            }
            if let Ok(reply) = result.extract::<String>() {
                return Ok(Some(reply));
            }
            if let Ok(raw) = result.extract::<Vec<u8>>() {
                return Ok(Some(String::from_utf8_lossy(&raw).into_owned()));
            }
            Err(PythonError::Py(
                "on_text_frame must return str, bytes, or None".to_string(),
            ))
        })
    }

    /// Dispatch a binary frame to `on_binary_frame`.
    ///
    /// Returns the handler's optional reply (`bytes`, or `str` encoded as utf-8).
    pub fn on_binary_frame(&self, frame: &[u8]) -> Result<Option<Vec<u8>>, PythonError> {
        Python::attach(|py| -> Result<Option<Vec<u8>>, PythonError> {
            let module = self.module.bind(py);
            if !module.hasattr(HOOK_ON_BINARY_FRAME)? {
                return Ok(None);
            }
            let frame_obj = PyBytes::new(py, frame);
            let result = module.call_method1(HOOK_ON_BINARY_FRAME, (frame_obj,))?;
            if result.is_none() {
                return Ok(None);
            }
            if let Ok(raw) = result.extract::<Vec<u8>>() {
                return Ok(Some(raw));
            }
            if let Ok(reply) = result.extract::<String>() {
                return Ok(Some(reply.into_bytes()));
            }
            Err(PythonError::Py(
                "on_binary_frame must return bytes, str, or None".to_string(),
            ))
        })
    }

    /// Call the optional `on_shutdown` hook, best-effort.
    pub fn on_shutdown(&self) -> Result<(), PythonError> {
        Python::attach(|py| -> Result<(), PythonError> {
            let module = self.module.bind(py);
            if !module.hasattr(HOOK_ON_SHUTDOWN)? {
                return Ok(());
            }
            drop(module.call_method0(HOOK_ON_SHUTDOWN)?);
            Ok(())
        })
    }
}
