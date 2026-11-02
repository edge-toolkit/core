//! The hub beside the board's guests: each guest's websocket a link into it in the same process rather than a socket,
//! and its storage the memory the hub serves agents' storage from.

#[cfg(target_os = "none")]
use alloc::string::String;
#[cfg(target_os = "none")]
use alloc::vec::Vec;
use core::future::poll_fn;
use core::net::{IpAddr, Ipv4Addr};
use core::pin::pin;
use core::task::Poll;

use et_storage_service::picoserve::{NotStored, Storage};
use et_ws_service::hub::{SessionMessage, Stamps};
use et_ws_service::picoserve_hub::{FrameTooLarge, Hub, LocalLink};
use et_ws_wasi_runner::board::{Absent, Buckets, Platform, Transport};
use et_ws_wasi_runner::convert::Frame;

/// The address a runner's agent is registered from: the board itself.
const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
/// Nanoseconds in a millisecond, the unit a guest's `recv` timeout comes in.
const NANOS_PER_MILLI: u64 = 1_000_000;

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
    /// Why the link refused, as the guest is told it.
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
    buckets: Buckets,
}

impl<S, P> HubTransport<S, P> {
    /// A transport into `hub`, its storage `buckets`, waiting on `platform`'s clock, not yet open.
    #[expect(clippy::single_call_fn, reason = "the constructor, called where a guest is started")]
    pub(crate) const fn new(hub: &'static Hub<S>, platform: P, buckets: Buckets) -> Self {
        Self {
            hub,
            platform,
            state: State::Closed,
            buckets,
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

    /// What the board's storage holds under `key` in `bucket`, which is the hub's beside it.
    async fn load(&mut self, bucket: &str, key: &str) -> Result<Option<Vec<u8>>, LinkError> {
        Ok(self.buckets.get(bucket, key).ok())
    }

    /// Store `value` in the board's storage, which the hub beside it serves.
    async fn store(&mut self, bucket: &str, key: &str, value: Vec<u8>) -> Result<(), LinkError> {
        self.buckets.set(bucket, key, value);
        Ok(())
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

/// The board's storage as the hub serves it.
///
/// So an agent anywhere reads what a guest wrote, and a guest what it uploaded.
pub(crate) struct BoardStorage(Buckets);

impl BoardStorage {
    /// `buckets`, as the hub serves them.
    #[expect(
        clippy::single_call_fn,
        reason = "the constructor, called where the board's storage is shared"
    )]
    pub(crate) const fn new(buckets: Buckets) -> Self {
        Self(buckets)
    }
}

impl Storage for BoardStorage {
    /// What a guest, or an agent through the hub, stored under `filename` in `agent_id`'s bucket.
    fn get(&self, agent_id: &str, filename: &str) -> Result<Vec<u8>, NotStored> {
        match self.0.get(agent_id, filename) {
            Ok(contents) => Ok(contents),
            Err(Absent { .. }) => Err(NotStored),
        }
    }

    /// Store what an agent uploaded through the hub, where the board's guests read it too.
    fn set(&self, agent_id: &str, filename: &str, contents: Vec<u8>) {
        self.0.set(agent_id, filename, contents);
    }
}
