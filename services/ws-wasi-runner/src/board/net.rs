//! A hub on another machine, reached over the board's own sockets: its websocket, and its storage over HTTP.
//!
//! A guest's [`Transport`] has to be `Send`, and a board's socket usually is not, so the two halves run apart. The
//! guest holds a [`NetTransport`], which only queues requests on a [`NetLink`] and waits on the answers, and a task of
//! the board's drives the sockets with [`drive`], taking the requests off the same link. Each guest has a link and a
//! driving task of its own.

#![expect(
    clippy::single_call_fn,
    reason = "each step of driving the sockets, the handshake, the two directions and the response, is named apart"
)]

#[cfg(not(feature = "std"))]
use alloc::{format, string::String, vec, vec::Vec};
use core::fmt::{self, Write as _};
use core::future::poll_fn;
use core::pin::pin;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::Poll;

use edge_ws::io::send;
use edge_ws::{FrameHeader, FrameType};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embedded_io_async::{Read, Write};

use super::platform::{Platform, Transport};
use crate::convert::Frame;

/// The largest frame the hub may send a guest, which bounds what one takes of a board's heap.
pub const MAX_FRAME: usize = 64 * 1024;
/// The largest HTTP response a storage request reads: a stored file, at most the hub's limit, and its head.
const MAX_RESPONSE: usize = 68 * 1024;
/// How long a websocket goes without a frame from the board before the board pings it, inside the hub's 15 seconds.
const KEEP_ALIVE_NANOS: u64 = 5_000_000_000;
/// Nanoseconds in a millisecond, the unit a guest's `recv` timeout comes in.
const NANOS_PER_MILLI: u64 = 1_000_000;
/// The path the hub serves its websocket on.
const WS_PATH: &str = "/ws";
/// The longest response head a hub's upgrade answers with.
const MAX_HEAD: usize = 1024;

/// Why a request to the hub failed, which the guest is told.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum NetError {
    /// No socket to the hub could be opened, for the reason given.
    Connect(String),
    /// The hub did not upgrade the connection to a websocket.
    Handshake,
    /// The websocket is not open, or the hub closed it.
    Closed,
    /// The socket failed while it was in use.
    Io,
    /// A frame or response larger than a board reads.
    TooLarge,
    /// A response that is not HTTP.
    Malformed,
    /// The hub answered a storage request with this status.
    Status(u16),
}

impl fmt::Display for NetError {
    /// What failed, as the guest is told it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(reason) => write!(f, "could not reach the hub: {reason}"),
            Self::Handshake => f.write_str("the hub did not open a websocket"),
            Self::Closed => f.write_str("the websocket to the hub is closed"),
            Self::Io => f.write_str("the connection to the hub failed"),
            Self::TooLarge => f.write_str("the hub sent more than a board reads"),
            Self::Malformed => f.write_str("the hub's answer was not HTTP"),
            Self::Status(status) => write!(f, "the hub answered {status}"),
        }
    }
}

/// What a guest asks of the task driving its sockets.
enum Request {
    /// Open the websocket.
    Connect,
    /// Send a frame on the websocket.
    Send(Frame),
    /// Answer the hub's ping, with what it carried.
    Pong(Vec<u8>),
    /// Close the websocket.
    Close,
    /// Read `key` in `bucket` from the hub's storage.
    Load { bucket: String, key: String },
    /// Write `value` under `key` in `bucket` to the hub's storage.
    Store {
        bucket: String,
        key: String,
        value: Vec<u8>,
    },
}

/// The answer to a request that has one.
enum Reply {
    /// The websocket opened, or why not.
    Connected(Result<(), NetError>),
    /// What a `Load` read.
    Loaded(Result<Option<Vec<u8>>, NetError>),
    /// Whether a `Store` was taken.
    Stored(Result<(), NetError>),
}

/// The queues between one guest's [`NetTransport`] and the task driving its sockets.
///
/// `static` on a board, since both sides hold it for as long as the board runs.
pub struct NetLink {
    requests: Channel<CriticalSectionRawMutex, Request, 1>,
    replies: Channel<CriticalSectionRawMutex, Reply, 1>,
    incoming: Channel<CriticalSectionRawMutex, Result<Frame, NetError>, 4>,
    open: AtomicBool,
}

impl NetLink {
    /// A link with nothing queued and no websocket open, made in a `const` so a board can keep it in a `static`.
    #[must_use]
    pub const fn idle() -> Self {
        Self {
            requests: Channel::new(),
            replies: Channel::new(),
            incoming: Channel::new(),
            open: AtomicBool::new(false),
        }
    }

    /// Ask the driving task for `request`, and wait for its answer.
    async fn ask(&self, request: Request) -> Reply {
        self.requests.send(request).await;
        self.replies.receive().await
    }
}

/// A guest's [`Transport`] to a hub on another machine, through the task that drives `link`.
pub struct NetTransport<P> {
    link: &'static NetLink,
    platform: P,
}

impl<P> NetTransport<P> {
    /// A transport through `link`, waiting on `platform`'s clock.
    #[must_use]
    pub const fn new(link: &'static NetLink, platform: P) -> Self {
        Self { link, platform }
    }
}

impl<P: Platform> Transport for NetTransport<P> {
    type Error = NetError;

    /// Open the websocket, dropping whatever an earlier one left unread.
    async fn connect(&mut self) -> Result<(), NetError> {
        self.link.incoming.clear();
        match self.link.ask(Request::Connect).await {
            Reply::Connected(outcome) => outcome,
            Reply::Loaded(_) | Reply::Stored(_) => Err(NetError::Malformed),
        }
    }

    /// Queue `frame` to send; a failure to send it closes the websocket, which the next `recv` reports.
    async fn send(&mut self, frame: Frame) -> Result<(), NetError> {
        if !self.link.open.load(Ordering::Acquire) {
            return Err(NetError::Closed);
        }
        self.link.requests.send(Request::Send(frame)).await;
        Ok(())
    }

    /// The next frame from the hub, if one comes within `timeout_ms`.
    async fn recv(&mut self, timeout_ms: u32) -> Result<Option<Frame>, NetError> {
        let deadline = self
            .platform
            .now()
            .saturating_add(u64::from(timeout_ms).saturating_mul(NANOS_PER_MILLI));
        match race(self.link.incoming.receive(), self.platform.sleep_until(deadline)).await {
            Won::First(Ok(frame)) => Ok(Some(frame)),
            Won::First(Err(error)) => Err(error),
            Won::Second(()) => Ok(None),
        }
    }

    /// Close the websocket.
    async fn close(&mut self) {
        if self.link.open.load(Ordering::Acquire) {
            self.link.requests.send(Request::Close).await;
        }
    }

    /// What the hub stores under `key` in `bucket`, as `GET /storage/{bucket}/{key}` answers.
    async fn load(&mut self, bucket: &str, key: &str) -> Result<Option<Vec<u8>>, NetError> {
        let request = Request::Load {
            bucket: String::from(bucket),
            key: String::from(key),
        };
        match self.link.ask(request).await {
            Reply::Loaded(outcome) => outcome,
            Reply::Connected(_) | Reply::Stored(_) => Err(NetError::Malformed),
        }
    }

    /// Store `value` under `key` in `bucket` with `PUT /storage/{bucket}/{key}`.
    async fn store(&mut self, bucket: &str, key: &str, value: Vec<u8>) -> Result<(), NetError> {
        let request = Request::Store {
            bucket: String::from(bucket),
            key: String::from(key),
            value,
        };
        match self.link.ask(request).await {
            Reply::Stored(outcome) => outcome,
            Reply::Connected(_) | Reply::Loaded(_) => Err(NetError::Malformed),
        }
    }
}

/// Opens a new socket to the hub each time it is asked, with buffers of its own.
pub trait Connector {
    /// The socket, borrowing what the connector keeps for it.
    type Socket<'socket>: Read + Write + Split
    where
        Self: 'socket;

    /// A socket to the hub, freshly connected.
    ///
    /// # Errors
    ///
    /// [`NetError::Connect`], saying why the hub could not be reached, as the guest is told it.
    fn connect(&mut self) -> impl Future<Output = Result<Self::Socket<'_>, NetError>>;
}

/// A socket that can be read and written at once, from two halves.
pub trait Split {
    /// The half that reads.
    type Reader<'half>: Read
    where
        Self: 'half;
    /// The half that writes.
    type Writer<'half>: Write
    where
        Self: 'half;

    /// The two halves, borrowing the socket.
    fn split(&mut self) -> (Self::Reader<'_>, Self::Writer<'_>);
}

/// Answer what the guest on `link` asks, forever, over sockets to the hub at `host`.
///
/// `host` is the hub's `address:port`. The websocket's socket comes from `websocket`, and each storage request's from
/// `http`, so a request can be made while the websocket is open. The keep-alive and the frames' masks are timed on
/// `platform`'s clock.
pub async fn drive<W, H, P>(link: &NetLink, mut websocket: W, mut http: H, host: &str, platform: P) -> !
where
    W: Connector,
    H: Connector,
    P: Platform,
{
    loop {
        match link.requests.receive().await {
            Request::Connect => match websocket.connect().await {
                Ok(mut socket) => match handshake(&mut socket, host).await {
                    Ok(()) => {
                        link.open.store(true, Ordering::Release);
                        link.replies.send(Reply::Connected(Ok(()))).await;
                        session(link, &mut socket, &mut http, host, &platform).await;
                        link.open.store(false, Ordering::Release);
                    }
                    Err(error) => link.replies.send(Reply::Connected(Err(error))).await,
                },
                Err(refused) => link.replies.send(Reply::Connected(Err(refused))).await,
            },
            Request::Load { bucket, key } => {
                let loaded = load(&mut http, host, &bucket, &key).await;
                link.replies.send(Reply::Loaded(loaded)).await;
            }
            Request::Store { bucket, key, value } => {
                let stored = store(&mut http, host, &bucket, &key, &value).await;
                link.replies.send(Reply::Stored(stored)).await;
            }
            // With no websocket open there is nothing to send on or close.
            Request::Send(_) | Request::Pong(_) | Request::Close => {}
        }
    }
}

/// Upgrade `socket` to a websocket on the hub at `host`.
async fn handshake<S>(socket: &mut S, host: &str) -> Result<(), NetError>
where
    S: Read + Write,
{
    // The key is only there to be echoed back: a hub checks it is 16 bytes of base64, not that it is random, and a
    // board has no use for the answer. Sixteen zero bytes it is.
    let request = format!(
        concat!(
            "GET {path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n",
            "Sec-WebSocket-Key: AAAAAAAAAAAAAAAAAAAAAA==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        ),
        path = WS_PATH,
        host = host,
    );
    socket.write_all(request.as_bytes()).await.map_net_err()?;
    let head = read_head(socket).await?;
    if head.starts_with(b"HTTP/1.1 101") {
        Ok(())
    } else {
        Err(NetError::Handshake)
    }
}

/// The response head `socket` sends, up to and including the blank line that ends it.
///
/// Read a byte at a time, so that nothing after it is taken.
async fn read_head<S>(socket: &mut S) -> Result<Vec<u8>, NetError>
where
    S: Read,
{
    let mut head = Vec::new();
    let mut byte = [0];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() >= MAX_HEAD {
            return Err(NetError::TooLarge);
        }
        socket.read_exact(&mut byte).await.map_net_err()?;
        head.push(byte[0]);
    }
    Ok(head)
}

/// Carry frames both ways on `socket` until the hub or the guest closes it, keeping the websocket alive.
///
/// The hub's frames are read throughout. Only the wait for what the guest asks next races that reading: once a request
/// is taken off the queue it runs to its end, a storage request's HTTP and reply included, so a websocket that ends
/// meanwhile cannot drop a request the guest is still waiting on.
async fn session<S, H, P>(link: &NetLink, socket: &mut S, http: &mut H, host: &str, platform: &P)
where
    S: Split,
    H: Connector,
    P: Platform,
{
    let (reader, mut writer) = socket.split();
    let mut reading = pin!(read_frames(link, reader));
    loop {
        let now = platform.now();
        let idle = now.saturating_add(KEEP_ALIVE_NANOS);
        let next = race(link.requests.receive(), platform.sleep_until(idle));
        let Won::Second(next) = race(reading.as_mut(), next).await else {
            return;
        };
        // The mask is the clock's low bits: a board has no randomness to spare, and a hub needs none.
        let mask = Some(u32::try_from(now & u64::from(u32::MAX)).unwrap_or_default());
        let sent = match next {
            Won::Second(()) => send(&mut writer, FrameType::Ping, mask, &[]).await,
            Won::First(request) => match answer(link, request, &mut writer, mask, http, host).await {
                Answered::Continue(sent) => sent,
                Answered::Closed => return,
            },
        };
        if sent.is_err() {
            link.incoming.send(Err(NetError::Io)).await;
            return;
        }
    }
}

/// What answering one of the guest's requests left the websocket as.
enum Answered<E> {
    /// Still open, with whether what was sent on it went.
    Continue(Result<(), edge_ws::Error<E>>),
    /// Closed by the guest.
    Closed,
}

/// Answer `request` while the websocket is open: send on `writer` what it sends, and reply to what it asks.
async fn answer<W, H>(
    link: &NetLink,
    request: Request,
    writer: &mut W,
    mask: Option<u32>,
    http: &mut H,
    host: &str,
) -> Answered<W::Error>
where
    W: Write,
    H: Connector,
{
    Answered::Continue(match request {
        Request::Send(Frame::Text(text)) => send(writer, FrameType::Text(false), mask, text.as_bytes()).await,
        Request::Send(Frame::Binary(bytes)) => send(writer, FrameType::Binary(false), mask, &bytes).await,
        Request::Pong(payload) => send(writer, FrameType::Pong, mask, &payload).await,
        Request::Close => {
            let _closing = send(writer, FrameType::Close, mask, &[]).await;
            return Answered::Closed;
        }
        Request::Connect => {
            link.replies.send(Reply::Connected(Ok(()))).await;
            Ok(())
        }
        Request::Load { bucket, key } => {
            let loaded = load(http, host, &bucket, &key).await;
            link.replies.send(Reply::Loaded(loaded)).await;
            Ok(())
        }
        Request::Store { bucket, key, value } => {
            let stored = store(http, host, &bucket, &key, &value).await;
            link.replies.send(Reply::Stored(stored)).await;
            Ok(())
        }
    })
}

/// Hand every frame the hub sends to the guest, answering its pings, until the websocket ends.
async fn read_frames<R>(link: &NetLink, mut reader: R)
where
    R: Read,
{
    let mut message = Vec::new();
    // Whether the message being read is binary, which only its first frame says: the rest are continuations.
    let mut binary = false;
    let ended = loop {
        let header = match FrameHeader::recv(&mut reader).await {
            Ok(header) => header,
            Err(_failed) => break NetError::Io,
        };
        let Some(length) = usize::try_from(header.payload_len)
            .ok()
            .filter(|length| message.len().saturating_add(*length) <= MAX_FRAME)
        else {
            break NetError::TooLarge;
        };
        let mut payload = vec![0; length];
        if header.recv_payload(&mut reader, &mut payload).await.is_err() {
            break NetError::Io;
        }
        match header.frame_type {
            FrameType::Ping => link.requests.send(Request::Pong(payload)).await,
            FrameType::Pong => {}
            FrameType::Close => break NetError::Closed,
            FrameType::Text(_) | FrameType::Binary(_) | FrameType::Continue(_) => {
                if !matches!(header.frame_type, FrameType::Continue(_)) {
                    binary = matches!(header.frame_type, FrameType::Binary(_));
                }
                message.extend_from_slice(&payload);
                if header.frame_type.is_final() {
                    let frame = if binary {
                        Frame::Binary(core::mem::take(&mut message))
                    } else {
                        Frame::Text(String::from_utf8_lossy(&core::mem::take(&mut message)).into_owned())
                    };
                    link.incoming.send(Ok(frame)).await;
                }
            }
        }
    };
    link.incoming.send(Err(ended)).await;
}

/// What the hub stores under `key` in `bucket`: its body on 200, nothing on 404.
async fn load<H>(http: &mut H, host: &str, bucket: &str, key: &str) -> Result<Option<Vec<u8>>, NetError>
where
    H: Connector,
{
    match request(http, host, "GET", bucket, key, &[]).await? {
        (200, body) => Ok(Some(body)),
        (404, _) => Ok(None),
        (status, _) => Err(NetError::Status(status)),
    }
}

/// Store `value` under `key` in `bucket` on the hub, which answers 200 when it took it.
async fn store<H>(http: &mut H, host: &str, bucket: &str, key: &str, value: &[u8]) -> Result<(), NetError>
where
    H: Connector,
{
    match request(http, host, "PUT", bucket, key, value).await? {
        (200, _) => Ok(()),
        (status, _) => Err(NetError::Status(status)),
    }
}

/// Make `method` on `/storage/{bucket}/{key}` with `body`, on a socket of its own, and return the status and body.
async fn request<H>(
    http: &mut H,
    host: &str,
    method: &str,
    bucket: &str,
    key: &str,
    body: &[u8],
) -> Result<(u16, Vec<u8>), NetError>
where
    H: Connector,
{
    let mut socket = http.connect().await?;
    let head = format!(
        "{method} /storage/{}/{} HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        encoded(bucket),
        encoded(key),
        body.len()
    );
    socket.write_all(head.as_bytes()).await.map_net_err()?;
    socket.write_all(body).await.map_net_err()?;
    socket.flush().await.map_net_err()?;
    let mut response = Vec::new();
    let mut chunk = [0; 512];
    loop {
        let read = socket.read(&mut chunk).await.map_net_err()?;
        if read == 0 {
            break;
        }
        if response.len().saturating_add(read) > MAX_RESPONSE {
            return Err(NetError::TooLarge);
        }
        response.extend_from_slice(chunk.get(..read).unwrap_or_default());
    }
    parse_response(&response)
}

/// The status and body of a whole HTTP response.
fn parse_response(response: &[u8]) -> Result<(u16, Vec<u8>), NetError> {
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or(NetError::Malformed)?;
    let (head, rest) = response.split_at(split);
    let status = core::str::from_utf8(head)
        .ok()
        .and_then(|head| head.split(' ').nth(1))
        .and_then(|status| status.parse().ok())
        .ok_or(NetError::Malformed)?;
    Ok((status, rest.get(4..).unwrap_or_default().to_vec()))
}

/// `segment` as one path segment: every byte but the unreserved ones percent-encoded.
fn encoded(segment: &str) -> String {
    let mut out = String::default();
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            let _written = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// A socket call's failure, as the guest is told it: which error the socket gave says nothing a board can act on.
trait MapNetErr<T> {
    /// The value, or [`NetError::Io`].
    fn map_net_err(self) -> Result<T, NetError>;
}

impl<T, E> MapNetErr<T> for Result<T, E> {
    fn map_net_err(self) -> Result<T, NetError> {
        match self {
            Ok(value) => Ok(value),
            Err(_failed) => Err(NetError::Io),
        }
    }
}

/// Which of two raced futures finished first, and with what.
enum Won<A, B> {
    First(A),
    Second(B),
}

/// Poll `first` and `second` together until one finishes, dropping the other.
async fn race<A, B>(first: A, second: B) -> Won<A::Output, B::Output>
where
    A: Future,
    B: Future,
{
    let mut first = pin!(first);
    let mut second = pin!(second);
    poll_fn(|context| {
        if let Poll::Ready(output) = first.as_mut().poll(context) {
            return Poll::Ready(Won::First(output));
        }
        second.as_mut().poll(context).map(Won::Second)
    })
    .await
}
