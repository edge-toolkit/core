//! The wasi runners the firmware starts beside its hub: each guest the build compiled in, run on the board's engine,
//! its websocket a link into the hub in the same process rather than a socket.
//!
//! The board and the host share all of it but the clock and the log, which each hands in as a [`Platform`].

#[cfg(target_os = "none")]
use alloc::string::{String, ToString as _};
use core::future::poll_fn;
use core::net::{IpAddr, Ipv4Addr};
use core::pin::pin;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicPtr, Ordering};
use core::task::Poll;

use et_ws_service::hub::{SessionMessage, Stamps};
use et_ws_service::picoserve_hub::{FrameTooLarge, Hub, LocalLink};
use et_ws_wasi_runner::board::{BoardError, Buckets, MemoryBudget, Platform, Transport, run};
use et_ws_wasi_runner::convert::Frame;
use et_ws_wasi_runner::engine::board_engine;

include!(concat!(env!("OUT_DIR"), "/guests.rs"));

/// How many guest tasks a board's executor holds room for.
///
/// One per guest, and one for a build with none, since a pool cannot be empty.
#[cfg_attr(
    not(target_os = "none"),
    expect(dead_code, reason = "the host spawns its guests without a pool")
)]
pub(crate) const GUEST_POOL: usize = if GUESTS.is_empty() { 1 } else { GUESTS.len() };
/// The address a runner's agent is registered from: the board itself.
const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
/// Nanoseconds in a millisecond, the unit a guest's `recv` timeout comes in.
const NANOS_PER_MILLI: u64 = 1_000_000;

/// The pointers wasmtime keeps for the call it is in, which with one thread are statics rather than a thread's.
///
/// Slot 0 is its runtime state, and slot 1 its component-model-async state; it asks for no other.
static WASMTIME_TLS: [AtomicPtr<u8>; 2] = [AtomicPtr::new(null_mut()), AtomicPtr::new(null_mut())];

/// wasmtime's read of thread-local `slot`, which a build without `std` asks the embedder for.
#[expect(
    unsafe_code,
    reason = "wasmtime links against this name, so it has to be exported unmangled; nothing else defines it"
)]
#[unsafe(no_mangle)]
extern "C" fn wasmtime_tls_get(slot: usize) -> *mut u8 {
    WASMTIME_TLS
        .get(slot)
        .map_or(null_mut(), |pointer| pointer.load(Ordering::Relaxed))
}

/// wasmtime's write of thread-local `slot`, which a build without `std` asks the embedder for.
#[expect(
    unsafe_code,
    reason = "wasmtime links against this name, so it has to be exported unmangled; nothing else defines it"
)]
#[unsafe(no_mangle)]
extern "C" fn wasmtime_tls_set(slot: usize, pointer: *mut u8) {
    if let Some(stored) = WASMTIME_TLS.get(slot) {
        stored.store(pointer, Ordering::Relaxed);
    }
}

/// Why a guest's link to the hub refused a call; nothing between them can break, so only the guest's own use.
#[derive(Debug)]
pub(crate) enum LinkError {
    /// The link was used while closed.
    NotOpen,
    /// A frame larger than the hub reads from a socket, which it holds the board's own agents to too.
    TooLarge(FrameTooLarge),
}

impl From<FrameTooLarge> for LinkError {
    fn from(refused: FrameTooLarge) -> Self {
        Self::TooLarge(refused)
    }
}

impl core::fmt::Display for LinkError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotOpen => f.write_str("the link to the hub is not open"),
            Self::TooLarge(refused) => refused.fmt(f),
        }
    }
}

/// Whether a guest's link to the hub is open, and the link when it is.
enum State<S: 'static> {
    Closed,
    Open(LocalLink<'static, S>),
}

/// A guest's websocket to the hub it runs beside: a link in process, opened on the guest's `connect`.
///
/// Its waits are on the guest's own clock, `platform`'s. It needs no keep-alive, since nothing idles out a link.
pub(crate) struct HubTransport<S: 'static, P> {
    hub: &'static Hub<S>,
    platform: P,
    state: State<S>,
}

impl<S, P> HubTransport<S, P> {
    /// A transport into `hub`, waiting on `platform`'s clock, not yet open.
    #[expect(
        clippy::single_call_fn,
        reason = "the transport's constructor, called where a guest is started"
    )]
    pub(crate) const fn new(hub: &'static Hub<S>, platform: P) -> Self {
        Self {
            hub,
            platform,
            state: State::Closed,
        }
    }
}

#[expect(
    clippy::unused_async_trait_impl,
    reason = "the Transport trait declares every method async, and a link in process never waits to open or send"
)]
impl<S, P> Transport for HubTransport<S, P>
where
    S: Stamps + Send + 'static,
    P: Platform,
{
    type Error = LinkError;

    /// Join the hub as an agent of the board's own, which never fails, since nothing lies between them.
    async fn connect(&mut self) -> Result<(), LinkError> {
        self.state = State::Open(self.hub.local(LOOPBACK));
        Ok(())
    }

    /// Route `frame` through the hub, as a connection's frame would be, refusing one larger than a connection's may be.
    async fn send(&mut self, frame: Frame) -> Result<(), LinkError> {
        let State::Open(link) = &mut self.state else {
            return Err(LinkError::NotOpen);
        };
        match frame {
            Frame::Text(text) => link.send_text(&text)?,
            Frame::Binary(bytes) => link.send_binary(bytes.into())?,
            // Every kind of frame a guest sends is one of the two above.
            _ => {}
        }
        Ok(())
    }

    /// The next message for the guest, as the frame a socket would have carried, if one comes within `timeout_ms`.
    async fn recv(&mut self, timeout_ms: u32) -> Result<Option<Frame>, LinkError> {
        let State::Open(link) = &mut self.state else {
            return Err(LinkError::NotOpen);
        };
        let deadline = self
            .platform
            .now()
            .saturating_add(u64::from(timeout_ms).saturating_mul(NANOS_PER_MILLI));
        let Ok(message) = first(link.next(), self.platform.sleep_until(deadline)).await else {
            return Ok(None);
        };
        Ok(Some(match message {
            SessionMessage::Json(message) => Frame::Text(serde_json::to_string(&message).unwrap_or_default()),
            SessionMessage::Text(text) => Frame::Text(text),
            SessionMessage::Binary(bytes) => Frame::Binary(bytes.to_vec()),
            _ => Frame::Text(String::default()),
        }))
    }

    /// Leave the hub, which drops the link and so disconnects the guest's agent.
    async fn close(&mut self) {
        self.state = State::Closed;
    }
}

/// That a wait's deadline passed before what it waited for arrived.
struct Elapsed;

/// What `wanted` resolves to, or [`Elapsed`] once `deadline` has passed first.
#[expect(
    clippy::single_call_fn,
    reason = "the timeout of recv, a race of two futures kept apart from the frames"
)]
async fn first<W, D>(wanted: W, deadline: D) -> Result<W::Output, Elapsed>
where
    W: Future,
    D: Future<Output = ()>,
{
    let mut wanted = pin!(wanted);
    let mut deadline = pin!(deadline);
    poll_fn(|context| {
        if let Poll::Ready(output) = wanted.as_mut().poll(context) {
            return Poll::Ready(Ok(output));
        }
        deadline.as_mut().poll(context).map(|()| Err(Elapsed))
    })
    .await
}

/// What every guest on the board shares: its storage, and the memory the build holds for the guests.
#[derive(Clone)]
pub(crate) struct Shared {
    buckets: Buckets,
    memory: MemoryBudget,
}

impl Shared {
    /// The board's storage, empty, and all of [`GUEST_MEMORY`], none of it drawn on yet.
    #[expect(
        clippy::single_call_fn,
        reason = "made once, where the board's guests are started, as every guest has to share the one"
    )]
    pub(crate) fn new() -> Self {
        Self {
            buckets: Buckets::default(),
            memory: MemoryBudget::new(usize::try_from(GUEST_MEMORY).unwrap_or(usize::MAX)),
        }
    }
}

/// Run the guest `name` until it ends, against `hub`, on `platform`, with what it shares with the board's other guests.
///
/// `component` is the guest as the build compiled it in. How it ended is logged rather than returned, since nothing
/// waits on a runner: the hub carries on without it.
#[expect(
    clippy::single_call_fn,
    reason = "shared by the board's guest task and the host's, each of which a build has only one of"
)]
pub(crate) async fn run_guest<S, P>(hub: &'static Hub<S>, name: &str, component: &[u8], platform: P, shared: Shared)
where
    S: Stamps + Send + Sync + 'static,
    P: Platform + Clone + 'static,
{
    let transport = HubTransport::new(hub, platform.clone());
    let Shared { buckets, memory } = shared;
    let outcome = match board_engine() {
        Ok(engine) => run(&engine, component, platform.clone(), transport, buckets, memory).await,
        Err(error) => Err(BoardError::Wasmtime(error)),
    };
    let ended = match outcome {
        Ok(()) => "finished".to_string(),
        Err(error) => error.to_string(),
    };
    platform.write(
        et_ws_wasi_runner::board::Console::Stderr,
        alloc::format!("runner {name}: {ended}\n").as_bytes(),
    );
}
