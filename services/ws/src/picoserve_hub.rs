//! A `no_std` hub serving `/ws` over HTTP and WebSocket through picoserve, on whatever sockets the embedder accepts.
//!
//! The embedder owns the network stack and the executor. It accepts each connection on a socket picoserve can drive
//! -- an `embassy-net` `TcpSocket` on a microcontroller, a tokio `TcpStream` on a host -- and runs [`Hub::serve`] for
//! it, typically as a task of its own. Every connection shares the one [`Hub`], so agents on different sockets reach
//! each other through its registry, with [`Link`] deciding what goes where.
//!
//! It is a local hub: there is no backplane, admin API, idle timeout or registry file, and a frame larger than
//! [`Hub::new`]'s `max_frame_size` ends its connection.

use alloc::collections::VecDeque;
use alloc::sync::Arc;
#[cfg(not(feature = "std"))]
use alloc::vec;
use core::future::poll_fn;
use core::net::IpAddr;
use core::task::{Poll, Waker};

use bytes::Bytes;
use edge_toolkit::ws::ClientMessage;
use edge_toolkit::ws_server::{AgentRegistry, MessageRetention};
use picoserve::futures::Either;
use picoserve::io::{Read, Socket, Write};
use picoserve::response::StatusCode;
use picoserve::response::ws::{Message, ReadMessageError, SocketRx, SocketTx, WebSocketCallback, WebSocketUpgrade};
use picoserve::routing::{PathRouter, get};
use picoserve::time::Duration;
use picoserve::{Config, Router, Server, Timeouts, Timer};
use tracing::{error, info, warn};

use crate::hub::{Link, Routing, SessionMessage, Stamps};

/// How long a connection may last: picoserve's `read_request` bound, which also covers the upgraded WebSocket.
const CONNECTION_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

/// The picoserve configuration a hub connection needs.
///
/// picoserve bounds every read of a request by `read_request`, counted from the moment the request starts and still
/// running after the upgrade, so its default of three seconds would close each agent's WebSocket three seconds after
/// it connected. This lifts that bound to a day and leaves the other timeouts at picoserve's defaults; an embedder
/// wanting another maximum connection lifetime builds its own.
#[must_use]
pub const fn config() -> Config {
    Config::new(Timeouts {
        read_request: CONNECTION_LIFETIME,
        ..Timeouts::const_default()
    })
}

/// What a connection's agent has been sent by others and not yet written to its socket, and the task to wake for it.
#[derive(Debug)]
struct Mailbox {
    queue: VecDeque<SessionMessage>,
    max_queued: usize,
    waker: Option<Waker>,
}

/// The handle the registry holds for one connection: other agents' messages to it are queued here.
#[derive(Clone, Debug)]
pub struct Outbox(Arc<spin::Mutex<Mailbox>>);

impl Outbox {
    /// An empty outbox holding at most `max_queued` messages, though always the newest.
    ///
    /// The bound is what keeps a connection that stops reading from taking the heap with it: past it, the oldest
    /// message is dropped to make room.
    #[must_use]
    pub fn new(max_queued: usize) -> Self {
        Self(Arc::new(spin::Mutex::new(Mailbox {
            max_queued,
            queue: VecDeque::new(),
            waker: None,
        })))
    }

    /// Queue `message` for the connection, and wake it if it is waiting.
    pub fn send(&self, message: SessionMessage) {
        let waker = {
            let mut mailbox = self.0.lock();
            if mailbox.queue.len() >= mailbox.max_queued && mailbox.queue.pop_front().is_some() {
                warn!("Dropped the oldest queued message for a connection that is not reading");
            }
            mailbox.queue.push_back(message);
            mailbox.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    /// Whether `self` and `other` are handles to the same connection.
    #[must_use]
    pub fn same_connection(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// The next queued message, once there is one.
    ///
    /// Cancel-safe, as picoserve requires of the signal it races a frame read against: a message leaves the queue only
    /// in the poll that returns it, and the waker is stored under the same lock [`Outbox::send`] queues under.
    pub async fn next(&self) -> SessionMessage {
        poll_fn(|context| {
            let mut mailbox = self.0.lock();
            mailbox.queue.pop_front().map_or_else(
                || {
                    mailbox.waker = Some(context.waker().clone());
                    Poll::Pending
                },
                Poll::Ready,
            )
        })
        .await
    }
}

/// The hub every connection shares: its registry, the source of its ids and times, and its limits.
pub struct Hub<T> {
    registry: AgentRegistry<Outbox>,
    stamps: spin::Mutex<T>,
    retention: MessageRetention,
    max_frame_size: usize,
    max_queued: usize,
}

impl<T> Hub<T>
where
    T: Stamps,
{
    /// An empty hub taking ids and times from `stamps`, accepting frames of up to `max_frame_size` bytes.
    ///
    /// Each connection allocates a buffer of `max_frame_size` for the frames it reads, and queues at most `max_queued`
    /// of the messages other agents send it while it is still writing earlier ones, dropping the oldest past that.
    #[must_use]
    pub fn new(stamps: T, retention: MessageRetention, max_frame_size: usize, max_queued: usize) -> Self {
        Self {
            max_frame_size,
            max_queued,
            retention,
            stamps: spin::Mutex::new(stamps),
            registry: AgentRegistry::default(),
        }
    }

    /// The agents the hub knows, and the addresses it refuses.
    #[must_use]
    pub const fn registry(&self) -> &AgentRegistry<Outbox> {
        &self.registry
    }

    /// Serve one accepted connection from `client_ip` until it closes.
    ///
    /// `GET /ws` upgrades to the hub's WebSocket protocol, as [`Hub::routes`] describes; every other request is
    /// answered 404. `config` is usually [`config`], and `http_buffer` holds the request head.
    ///
    /// picoserve's futures nest deeply enough that the crate awaiting this one needs `#![recursion_limit = "256"]`
    /// for rustc to lay out the future; rustc says so when it is missing.
    #[expect(
        clippy::future_not_send,
        reason = "picoserve's futures hold Cells; its executors run each connection on one thread"
    )]
    pub async fn serve<R, M, S>(
        &self,
        socket: S,
        client_ip: IpAddr,
        timer: M,
        config: &Config,
        http_buffer: &mut [u8],
    ) -> Result<(), picoserve::Error<S::Error>>
    where
        M: Timer<R>,
        S: Socket<R>,
    {
        let app = self.routes(Router::new(), client_ip);
        let _disconnection = Server::custom(&app, timer, config, http_buffer).serve(socket).await?;
        Ok(())
    }

    /// `router`, with the hub's `GET /ws` added for a connection from `client_ip`.
    ///
    /// For a server that serves more than the hub on one socket: it builds its own router through this, and serves it
    /// as [`Hub::serve`] does. The upgrade is refused with 403 for an address the registry blocks.
    pub fn routes<P>(&self, router: Router<P>, client_ip: IpAddr) -> Router<impl PathRouter>
    where
        P: PathRouter,
    {
        router.route(
            "/ws",
            get(async move |upgrade: WebSocketUpgrade| {
                if self.registry.blocklist.is_blocked(client_ip) {
                    warn!("Refused a WebSocket connection from blocked IP {client_ip}");
                    return Err(StatusCode::FORBIDDEN);
                }
                Ok(upgrade.on_upgrade(Connection { hub: self, client_ip }))
            }),
        )
    }

    /// Route one frame from `link`'s agent.
    fn route(&self, link: &mut Link<Outbox>, frame: Frame) -> Routing<Outbox> {
        let mut stamps = self.stamps.lock();
        match frame {
            Frame::Message(message) => link.route(&self.registry, message, &mut *stamps),
            Frame::Binary(bytes) => link.route_binary(&self.registry, bytes, &mut *stamps),
        }
    }

    /// Exchange frames with `link`'s agent until it closes: route what it sends, and write what others send it.
    async fn exchange<R, W>(
        &self,
        link: &mut Link<Outbox>,
        rx: &mut SocketRx<R>,
        tx: &mut SocketTx<W>,
    ) -> Result<(), W::Error>
    where
        R: Read,
        W: Write<Error = R::Error>,
    {
        let outbox = link.session().clone();
        let mut buffer = vec![0; self.max_frame_size];
        loop {
            match rx.next_message(&mut buffer, outbox.next()).await? {
                Either::First(message) => match received(link, message) {
                    Received::Frame(frame) => deliver(self.route(link, frame), tx).await?,
                    Received::Ping(data) => tx.send_pong(data).await?,
                    Received::Ignored => {}
                    Received::Ended => return Ok(()),
                },
                Either::Second(queued) => write(tx, queued).await?,
            }
        }
    }
}

/// A frame from an agent the hub routes.
enum Frame {
    Message(ClientMessage),
    Binary(Bytes),
}

/// What one message read from an agent's socket asks of the connection.
enum Received<'buffer> {
    /// A frame to route.
    Frame(Frame),
    /// A ping to answer with these bytes.
    Ping(&'buffer [u8]),
    /// Nothing: a pong, or an `et-*` frame that did not decode.
    Ignored,
    /// The end of the connection: the agent closed it, or sent what could not be read.
    Ended,
}

/// What `message`, read from `link`'s agent, asks of the connection.
#[expect(
    clippy::cognitive_complexity,
    clippy::single_call_fn,
    reason = "the read step of exchange, kept out of its loop; the score is info!/warn! expansion"
)]
fn received<'buffer>(link: &Link<Outbox>, message: Result<Message<'buffer>, ReadMessageError>) -> Received<'buffer> {
    match message {
        Ok(Message::Text(text)) => match ClientMessage::from_text_frame(text) {
            Ok(message) => Received::Frame(Frame::Message(message)),
            Err(err) => {
                warn!("Decode error for et-* frame from {}: {}", link.current_agent_id(), err);
                Received::Ignored
            }
        },
        Ok(Message::Binary(bytes)) => Received::Frame(Frame::Binary(Bytes::copy_from_slice(bytes))),
        Ok(Message::Ping(data)) => Received::Ping(data),
        Ok(Message::Pong(_)) => Received::Ignored,
        Ok(Message::Close(reason)) => {
            info!(
                "WebSocket close request from client: {} reason: {:?}",
                link.current_agent_id(),
                reason
            );
            Received::Ended
        }
        Err(err) => {
            warn!(
                "Unreadable WebSocket message from {}: {:?}",
                link.current_agent_id(),
                err
            );
            Received::Ended
        }
    }
}

/// Carry out `routing`: other agents' messages go to their outboxes, and the replies down `tx`.
///
/// `routing.shared` is for other hubs, and this hub has none.
#[expect(clippy::single_call_fn, reason = "the write step of exchange, kept out of its loop")]
async fn deliver<W>(routing: Routing<Outbox>, tx: &mut SocketTx<W>) -> Result<(), W::Error>
where
    W: Write,
{
    for (session, message) in routing.deliveries {
        session.send(message);
    }
    for reply in routing.replies {
        write(tx, SessionMessage::Json(reply)).await?;
    }
    Ok(())
}

/// Write `message` to the agent at the far end of `tx`.
async fn write<W>(tx: &mut SocketTx<W>, message: SessionMessage) -> Result<(), W::Error>
where
    W: Write,
{
    match message {
        SessionMessage::Json(message) => match serde_json::to_string(&message) {
            Ok(text) => tx.send_text(&text).await,
            Err(err) => {
                error!("Failed to serialize websocket response: {}", err);
                Ok(())
            }
        },
        SessionMessage::Text(text) => tx.send_text(&text).await,
        SessionMessage::Binary(bytes) => tx.send_binary(&bytes).await,
    }
}

/// One upgraded WebSocket connection, from `client_ip`.
struct Connection<'hub, T> {
    hub: &'hub Hub<T>,
    client_ip: IpAddr,
}

impl<T> WebSocketCallback for Connection<'_, T>
where
    T: Stamps,
{
    #[expect(
        clippy::cognitive_complexity,
        reason = "the score is info! expansion; the body branches once"
    )]
    async fn run<R, W>(self, mut rx: SocketRx<R>, mut tx: SocketTx<W>) -> Result<(), W::Error>
    where
        R: Read,
        W: Write<Error = R::Error>,
    {
        info!("New WebSocket connection for client IP {}", self.client_ip);
        let mut link = Link::new(self.client_ip, Outbox::new(self.hub.max_queued), self.hub.retention);
        let exchanged = self.hub.exchange(&mut link, &mut rx, &mut tx).await;
        // Only this connection's own session: another may have taken the id over since, and is still live.
        let _released = link.release(&self.hub.registry, &|session| session.same_connection(link.session()));
        info!(
            "Agent {} disconnected; last known IP {}",
            link.current_agent_id(),
            self.client_ip
        );
        exchanged?;
        tx.close(None).await
    }
}
