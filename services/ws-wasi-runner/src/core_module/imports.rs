//! The `et` host module a core module imports, and carrying out the calls that pause it.
//!
//! An import that has to wait on the hub cannot wait inside the interpreter, which is synchronous. It reads its
//! arguments out of the guest's memory, records the call in [`Pending`], and pauses the interpreter; the run loop then
//! has [`resume`] carry the call out on the [`CoreHost`] and resume the guest with its result. `take` waits on
//! nothing, so it copies the result slot out inline.

#![expect(
    clippy::future_not_send,
    clippy::single_call_fn,
    reason = "SpaceWasm's engine is not Send, so no future driving it is; each import and call is a step named apart"
)]

use alloc::format;
use alloc::rc::Rc;
#[cfg(not(feature = "std"))]
use alloc::{string::String, vec::Vec};
use core::cell::RefCell;
use core::fmt::Debug;
use core::ops::ControlFlow;

use edge_toolkit::ws::ClientMessage;
use et_core_guest::abi;
use spacewasm::{Engine, HostFunction, HostFunctionBreak, HostModule, HostName, HostValList, Memory, Value};

use super::{CoreError, CoreHost};
use crate::bindings::et::ws_wasi::ws::WsError;
use crate::bindings::wasi::logging::logging::Level;

/// What a guest's log context is when the runner itself logs why one of its calls failed.
const LOG_CONTEXT: &str = env!("CARGO_PKG_NAME");

/// What the guest's imports and the run loop share: the call that paused the guest, and the host's result slot.
#[derive(Default)]
pub(super) struct Pending {
    /// The call the guest is paused in, until [`resume`] takes it.
    call: Option<Call>,
    /// The bytes the last call that produced any left for `take`.
    slot: Vec<u8>,
}

/// [`Pending`], shared by every import and the run loop, all on the one thread the engine runs on.
pub(super) type Shared = Rc<RefCell<Pending>>;

/// An import that waits on the host, with the arguments read out of the guest's memory.
enum Call {
    /// `connect`.
    Connect,
    /// `agent_id`.
    AgentId,
    /// `send` of the message JSON.
    Send(Vec<u8>),
    /// `recv`, waiting up to this many milliseconds.
    Recv(u32),
    /// `disconnect`.
    Disconnect,
    /// `kv_get`.
    KvGet {
        /// The bucket's name.
        bucket: String,
        /// The key in it.
        key: String,
    },
    /// `kv_set`.
    KvSet {
        /// The bucket's name.
        bucket: String,
        /// The key in it.
        key: String,
        /// The value to store.
        value: Vec<u8>,
    },
    /// `log`.
    Log {
        /// The entry's level.
        level: Level,
        /// The entry's context.
        context: String,
        /// The entry itself.
        message: String,
    },
}

/// An import the guest called with arguments that cannot be read, which traps the guest.
///
/// That is a pointer out of bounds, text that is not UTF-8, or an unknown log level.
struct Fault;

/// How an import that pauses reads its call out of the guest's memory and its arguments.
type ReadCall = fn(&Memory, &[Value]) -> Result<Call, Fault>;

/// The `et` host module, every import recording into or reading from `shared`.
pub(super) fn module(shared: &Shared) -> Result<HostModule, CoreError> {
    let functions = spacewasm::Vec::from_array([
        pausing(shared, abi::CONNECT, "", "i", |_, _| Ok(Call::Connect))?,
        pausing(shared, abi::AGENT_ID, "", "i", |_, _| Ok(Call::AgentId))?,
        pausing(shared, abi::SEND, "ii", "i", |memory, args| {
            Ok(Call::Send(bytes(memory, args, 0, 1)?))
        })?,
        pausing(shared, abi::RECV, "i", "i", |_, args| {
            Ok(Call::Recv(word(args, 0)?.cast_unsigned()))
        })?,
        take(shared)?,
        pausing(shared, abi::DISCONNECT, "", "", |_, _| Ok(Call::Disconnect))?,
        pausing(shared, abi::KV_GET, "iiii", "i", |memory, args| {
            Ok(Call::KvGet {
                bucket: text(memory, args, 0, 1)?,
                key: text(memory, args, 2, 3)?,
            })
        })?,
        pausing(shared, abi::KV_SET, "iiiiii", "i", |memory, args| {
            Ok(Call::KvSet {
                bucket: text(memory, args, 0, 1)?,
                key: text(memory, args, 2, 3)?,
                value: bytes(memory, args, 4, 5)?,
            })
        })?,
        pausing(shared, abi::LOG, "iiiii", "", |memory, args| {
            Ok(Call::Log {
                level: level(word(args, 0)?)?,
                context: text(memory, args, 1, 2)?,
                message: text(memory, args, 3, 4)?,
            })
        })?,
    ])?;
    Ok(HostModule {
        name: HostName::try_from_str(abi::MODULE)?,
        globals: spacewasm::Vec::zero(),
        functions,
        memory: spacewasm::Vec::zero(),
        table: spacewasm::Vec::zero(),
    })
}

/// The import `name`, taking `params` and returning `results`, which records the call `read` reads and pauses.
fn pausing(
    shared: &Shared,
    name: &str,
    params: &str,
    results: &str,
    read: ReadCall,
) -> Result<HostFunction, CoreError> {
    let shared = Rc::clone(shared);
    let signature = (HostValList::try_new(params)?, HostValList::try_new(results)?);
    Ok(HostFunction::try_new(
        HostName::try_from_str(name)?,
        signature.0,
        signature.1,
        move |engine, args| match read(&engine.memory, args) {
            Ok(call) => {
                shared.borrow_mut().call = Some(call);
                ControlFlow::Break(HostFunctionBreak::Pause)
            }
            Err(Fault) => ControlFlow::Break(HostFunctionBreak::Trap),
        },
    )?)
}

/// `take`, which copies the result slot into the guest's buffer when it fits, and returns the slot's length.
fn take(shared: &Shared) -> Result<HostFunction, CoreError> {
    let shared = Rc::clone(shared);
    Ok(HostFunction::try_new(
        HostName::try_from_str(abi::TAKE)?,
        HostValList::try_new("ii")?,
        HostValList::try_new("i")?,
        move |engine, args| match copy_slot(&engine.memory, args, &mut shared.borrow_mut().slot) {
            Ok(len) => ControlFlow::Continue(Some(Value::I32(len))),
            Err(Fault) => ControlFlow::Break(HostFunctionBreak::Trap),
        },
    )?)
}

/// Copy `slot` to the buffer `args` names in `memory` and empty it, when the buffer holds it; its length either way.
fn copy_slot(memory: &Memory, args: &[Value], slot: &mut Vec<u8>) -> Result<i32, Fault> {
    let len = i32::try_from(slot.len()).or(Err(Fault))?;
    if address(args, 1)? >= slot.len() {
        memory.store(address(args, 0)?, slot).or(Err(Fault))?;
        slot.clear();
    }
    Ok(len)
}

/// The `i32` argument at `index`.
fn word(args: &[Value], index: usize) -> Result<i32, Fault> {
    match args.get(index) {
        Some(Value::I32(value)) => Ok(*value),
        _ => Err(Fault),
    }
}

/// The argument at `index`, an address or a length in the guest's memory.
fn address(args: &[Value], index: usize) -> Result<usize, Fault> {
    usize::try_from(word(args, index)?.cast_unsigned()).or(Err(Fault))
}

/// The bytes in `memory` at the address the argument `pointer` gives, as many as the argument `length` gives.
fn bytes(memory: &Memory, args: &[Value], pointer: usize, length: usize) -> Result<Vec<u8>, Fault> {
    let loaded = memory.load(address(args, pointer)?, address(args, length)?);
    Ok(loaded.or(Err(Fault))?.to_vec())
}

/// The UTF-8 text [`bytes`] reads.
fn text(memory: &Memory, args: &[Value], pointer: usize, length: usize) -> Result<String, Fault> {
    String::from_utf8(bytes(memory, args, pointer, length)?).or(Err(Fault))
}

/// The `wasi:logging` level `code` names.
const fn level(code: i32) -> Result<Level, Fault> {
    match code {
        abi::LOG_TRACE => Ok(Level::Trace),
        abi::LOG_DEBUG => Ok(Level::Debug),
        abi::LOG_INFO => Ok(Level::Info),
        abi::LOG_WARN => Ok(Level::Warn),
        abi::LOG_ERROR => Ok(Level::Error),
        abi::LOG_CRITICAL => Ok(Level::Critical),
        _ => Err(Fault),
    }
}

/// Carry out on `host` the call the guest on `engine` paused in, and resume the guest with what the import returns.
///
/// # Errors
///
/// When the engine refuses the result, which it does only for one of another type than the import declared.
pub(super) async fn resume<H>(host: &mut H, shared: &Shared, engine: &mut Engine) -> Result<(), CoreError>
where
    H: CoreHost,
{
    let call = shared.borrow_mut().call.take();
    match call {
        Some(call) => perform(call, host, shared, engine).await,
        None => Ok(engine.resume(None)?),
    }
}

/// Carry `call` out on `host`, and resume the guest paused in it, on `engine`, with what the import returns.
async fn perform<H>(call: Call, host: &mut H, shared: &Shared, engine: &mut Engine) -> Result<(), CoreError>
where
    H: CoreHost,
{
    let code = match call {
        Call::Connect => {
            let connected = host.connect().await;
            ws_status(host, abi::CONNECT, connected).await
        }
        Call::AgentId => {
            let agent_id = host.agent_id().await;
            fill(shared, agent_id.into_bytes())
        }
        Call::Send(json) => send(host, &json).await,
        Call::Recv(timeout_ms) => recv(host, shared, timeout_ms).await,
        Call::Disconnect => {
            host.disconnect().await;
            return Ok(engine.resume(None)?);
        }
        Call::KvGet { bucket, key } => match host.get(&bucket, &key).await {
            Ok(Some(value)) => {
                let _len = fill(shared, value);
                abi::FOUND
            }
            Ok(None) => abi::ABSENT,
            Err(error) => failed(host, abi::KV_GET, &error, abi::STORE).await,
        },
        Call::KvSet { bucket, key, value } => match host.set(&bucket, &key, value).await {
            Ok(()) => abi::SUCCESS,
            Err(error) => failed(host, abi::KV_SET, &error, abi::STORE).await,
        },
        Call::Log {
            level,
            context,
            message,
        } => {
            host.log(level, &context, &message).await;
            return Ok(engine.resume(None)?);
        }
    };
    Ok(engine.resume(Some(Value::I32(code)))?)
}

/// `send` of the message `json` is: the message, unless it is not a `ClientMessage`.
async fn send<H>(host: &mut H, json: &[u8]) -> i32
where
    H: CoreHost,
{
    match serde_json::from_slice::<ClientMessage>(json) {
        Ok(message) => {
            let sent = host.send(message).await;
            ws_status(host, abi::SEND, sent).await
        }
        Err(error) => failed(host, abi::SEND, &error, abi::DECODE).await,
    }
}

/// `recv`: the next message's JSON into the slot, and its length, or [`abi::NOTHING`] when none arrived.
async fn recv<H>(host: &mut H, shared: &Shared, timeout_ms: u32) -> i32
where
    H: CoreHost,
{
    match host.recv(timeout_ms).await {
        Ok(Some(message)) => match serde_json::to_vec(&message) {
            Ok(json) => fill(shared, json),
            Err(error) => failed(host, abi::RECV, &error, abi::DECODE).await,
        },
        Ok(None) => abi::NOTHING,
        Err(error) => {
            let code = ws_code(&error);
            failed(host, abi::RECV, &error, code).await
        }
    }
}

/// Leave `bytes` in the slot for `take`, returning their length.
fn fill(shared: &Shared, bytes: Vec<u8>) -> i32 {
    // A guest's memory is at most 4 GiB, so a slot it has to fit fits an `i32` length or could never be taken.
    let len = i32::try_from(bytes.len()).unwrap_or(i32::MAX);
    shared.borrow_mut().slot = bytes;
    len
}

/// What a websocket call's `result` returns to the guest.
async fn ws_status<H>(host: &mut H, name: &str, result: Result<(), WsError>) -> i32
where
    H: CoreHost,
{
    match result {
        Ok(()) => abi::SUCCESS,
        Err(error) => {
            let code = ws_code(&error);
            failed(host, name, &error, code).await
        }
    }
}

/// The code a websocket `error` returns to the guest.
const fn ws_code(error: &WsError) -> i32 {
    match error {
        WsError::Transport(_) => abi::TRANSPORT,
        WsError::Decode(_) => abi::DECODE,
        WsError::NotConnected => abi::NOT_CONNECTED,
        WsError::AlreadyConnected => abi::ALREADY_CONNECTED,
    }
}

/// Log why the import `name` failed with `error`, and return `code` to the guest, which is told no more than that.
async fn failed<H, E>(host: &mut H, name: &str, error: &E, code: i32) -> i32
where
    H: CoreHost,
    E: Debug,
{
    host.log(Level::Warn, LOG_CONTEXT, &format!("{name} failed: {error:?}"))
        .await;
    code
}
