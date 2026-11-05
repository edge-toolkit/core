//! The typed guest API over the `et` imports.

use alloc::string::{FromUtf8Error, String, ToString as _};
use alloc::vec;
use alloc::vec::Vec;

use edge_toolkit::ws::{ClientMessage, ServerMessage};

use crate::abi;

/// The `et` imports as a guest calls them, each returning what its import returns.
///
/// Built for a Wasm 1.0 target, `wasm::Host` is the imports themselves; anything else stands in for them.
pub trait Imports {
    /// `connect`.
    fn connect(&mut self) -> i32;
    /// `agent_id`.
    fn agent_id(&mut self) -> i32;
    /// `send` of the `ClientMessage` JSON `message`.
    fn send(&mut self, message: &[u8]) -> i32;
    /// `recv`, waiting up to `timeout_ms`.
    fn recv(&mut self, timeout_ms: u32) -> i32;
    /// `take` into `buffer`.
    fn take(&mut self, buffer: &mut [u8]) -> i32;
    /// `disconnect`.
    fn disconnect(&mut self);
    /// `kv_get` of `key` in `bucket`.
    fn kv_get(&mut self, bucket: &str, key: &str) -> i32;
    /// `kv_set` of `value` under `key` in `bucket`.
    fn kv_set(&mut self, bucket: &str, key: &str, value: &[u8]) -> i32;
    /// `log` of `message` at the level `level`, under `context`.
    fn log(&mut self, level: i32, context: &str, message: &str);
}

/// Why a guest's call failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GuestError {
    /// The websocket failed, or the hub could not be reached.
    #[error("the websocket failed, or the hub could not be reached")]
    Transport,
    /// A message was not the JSON of the message it had to be.
    #[error("the host could not decode the message")]
    Decode,
    /// The websocket is not open.
    #[error("the websocket is not open")]
    NotConnected,
    /// The websocket is already open.
    #[error("the websocket is already open")]
    AlreadyConnected,
    /// The hub's storage refused the request, or failed.
    #[error("the hub's storage refused the request, or failed")]
    Store,
    /// The host answered with a code no `et` import returns.
    #[error("the host answered {0}, which no `et` import returns")]
    Unknown(i32),
    /// What the host handed back was not the JSON of the message it had to be.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// What the host handed back as text was not UTF-8.
    #[error(transparent)]
    Utf8(#[from] FromUtf8Error),
    /// The guest's own work failed, for the reason given.
    #[error("{0}")]
    Runtime(String),
}

impl GuestError {
    /// The code a guest's `run` returns when it fails with this error.
    #[must_use]
    pub const fn code(&self) -> i32 {
        match self {
            Self::Transport => abi::TRANSPORT,
            Self::Decode => abi::DECODE,
            Self::NotConnected => abi::NOT_CONNECTED,
            Self::AlreadyConnected => abi::ALREADY_CONNECTED,
            Self::Store => abi::STORE,
            Self::Unknown(code) => *code,
            Self::Json(_) | Self::Utf8(_) | Self::Runtime(_) => abi::FAILED,
        }
    }
}

/// A `wasi:logging` level, as `log` takes it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Level {
    /// Trace.
    Trace,
    /// Debug.
    Debug,
    /// Info.
    Info,
    /// Warn.
    Warn,
    /// Error.
    Error,
    /// Critical.
    Critical,
}

impl Level {
    /// The level's code, as `log` takes it.
    #[must_use]
    pub const fn code(self) -> i32 {
        match self {
            Self::Trace => abi::LOG_TRACE,
            Self::Debug => abi::LOG_DEBUG,
            Self::Info => abi::LOG_INFO,
            Self::Warn => abi::LOG_WARN,
            Self::Error => abi::LOG_ERROR,
            Self::Critical => abi::LOG_CRITICAL,
        }
    }
}

/// A guest's typed view of its imports, logging under the context it was made with.
#[derive(Debug)]
pub struct Guest<I> {
    /// The imports every call goes through.
    imports: I,
    /// What every log entry is recorded under: the guest's name.
    context: String,
}

impl<I: Imports> Guest<I> {
    /// A guest calling `imports`, logging under `context`.
    #[must_use]
    pub fn new(imports: I, context: &str) -> Self {
        Self {
            imports,
            context: context.to_string(),
        }
    }

    /// Log that the guest started, connect to the hub, and return the agent id it assigned.
    ///
    /// # Errors
    ///
    /// When the hub could not be reached, or the id it assigned could not be read.
    pub fn start(&mut self) -> Result<String, GuestError> {
        self.info("entered run()");
        self.connect()?;
        self.agent_id()
    }

    /// Open the websocket and register with the hub.
    ///
    /// # Errors
    ///
    /// When the hub could not be reached, or the websocket is already open.
    pub fn connect(&mut self) -> Result<(), GuestError> {
        let _code: i32 = status(self.imports.connect())?;
        Ok(())
    }

    /// The agent id the hub assigned.
    ///
    /// # Errors
    ///
    /// When the websocket is not open, or the id is not UTF-8.
    pub fn agent_id(&mut self) -> Result<String, GuestError> {
        let _len: i32 = status(self.imports.agent_id())?;
        Ok(String::from_utf8(self.take()?)?)
    }

    /// Send `message` to the hub.
    ///
    /// # Errors
    ///
    /// When the websocket is not open or failed.
    pub fn send(&mut self, message: &ClientMessage) -> Result<(), GuestError> {
        let json = serde_json::to_vec(message)?;
        let _code: i32 = status(self.imports.send(&json))?;
        Ok(())
    }

    /// The next message from the hub, waiting up to `timeout_ms`; `None` when none arrived in that time.
    ///
    /// # Errors
    ///
    /// When the websocket is not open or failed, or the message could not be read.
    pub fn recv(&mut self, timeout_ms: u32) -> Result<Option<ServerMessage>, GuestError> {
        if status(self.imports.recv(timeout_ms))? == abi::NOTHING {
            return Ok(None);
        }
        Ok(Some(serde_json::from_slice(&self.take()?)?))
    }

    /// Close the websocket.
    pub fn disconnect(&mut self) {
        self.imports.disconnect();
    }

    /// The value stored under `key` in `bucket`; `None` when the bucket holds nothing under it.
    ///
    /// # Errors
    ///
    /// When the hub's storage refused the request, or failed.
    pub fn get(&mut self, bucket: &str, key: &str) -> Result<Option<Vec<u8>>, GuestError> {
        if status(self.imports.kv_get(bucket, key))? == abi::ABSENT {
            return Ok(None);
        }
        Ok(Some(self.take()?))
    }

    /// Store `value` under `key` in `bucket`, replacing what was there.
    ///
    /// # Errors
    ///
    /// When the hub's storage refused the request, or failed.
    pub fn set(&mut self, bucket: &str, key: &str, value: &[u8]) -> Result<(), GuestError> {
        let _code: i32 = status(self.imports.kv_set(bucket, key, value))?;
        Ok(())
    }

    /// Record `message` at `level`.
    pub fn log(&mut self, level: Level, message: &str) {
        self.imports.log(level.code(), &self.context, message);
    }

    /// Record `message` at [`Level::Info`].
    pub fn info(&mut self, message: &str) {
        self.log(Level::Info, message);
    }

    /// The code the guest's `run` returns for `result`, logging why when it failed.
    pub fn finish(&mut self, result: Result<(), GuestError>) -> i32 {
        match result {
            Ok(()) => abi::SUCCESS,
            Err(error) => {
                self.log(Level::Error, &error.to_string());
                error.code()
            }
        }
    }

    /// Copy the host's result slot out: ask its length, then take it into a buffer that size.
    fn take(&mut self) -> Result<Vec<u8>, GuestError> {
        let len = length(self.imports.take(&mut []))?;
        let mut buffer = vec![0; len];
        let copied = self.imports.take(&mut buffer);
        if length(copied)? == len {
            Ok(buffer)
        } else {
            Err(GuestError::Unknown(copied))
        }
    }
}

/// What an import's return `code` says: the non-negative result, or the failure a negative code names.
const fn status(code: i32) -> Result<i32, GuestError> {
    match code {
        abi::TRANSPORT => Err(GuestError::Transport),
        abi::DECODE => Err(GuestError::Decode),
        abi::NOT_CONNECTED => Err(GuestError::NotConnected),
        abi::ALREADY_CONNECTED => Err(GuestError::AlreadyConnected),
        abi::STORE => Err(GuestError::Store),
        ..0 => Err(GuestError::Unknown(code)),
        _ => Ok(code),
    }
}

/// The length an import's return `code` gives, or the failure a negative code names.
fn length(code: i32) -> Result<usize, GuestError> {
    // A non-negative `i32` fits a `usize` on every target a guest or its tests build for, so the fallback is never
    // taken; it stands in for a conversion the standard library offers only as fallible.
    Ok(usize::try_from(status(code)?).unwrap_or(usize::MAX))
}
