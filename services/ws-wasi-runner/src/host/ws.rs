//! Implements `et:ws-wasi/ws` using `tokio-tungstenite`.
//!
//! On `connect`, we open a websocket, send `ClientMessage::Connect { agent_id: None }`, and spawn a task that pumps
//! inbound text messages into a channel. Inbound `connect_ack` messages capture our assigned `agent_id`. Messages
//! cross the WIT boundary as [`crate::convert`] converts them.

use std::sync::Arc;
use std::time::Duration;

use edge_toolkit::ws::ServerMessage;
use futures_util::SinkExt as _;
use futures_util::stream::{SplitSink, StreamExt as _};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite};
use tracing::warn;

use crate::HostState;
use crate::bindings::et::ws_messages::messages::{
    ClientMessage as WitClientMessage, ServerMessage as WitServerMessage,
};
use crate::bindings::et::ws_wasi::ws::{Host, State, WsError};
use crate::convert::{Frame, client_frame, server_message, server_message_to_wit};

/// Send `frame` on the websocket open in `ws`.
///
/// # Errors
///
/// When the websocket is not open, or the frame could not be sent.
pub async fn send_frame(ws: &Mutex<Option<WsBackend>>, frame: Frame) -> Result<(), WsError> {
    let outgoing = match frame {
        Frame::Text(text) => tungstenite::Message::text(text),
        Frame::Binary(bytes) => tungstenite::Message::binary(bytes),
    };
    // Clone the sink Arc and release the outer lock before awaiting the inner send -- keeping `ws` locked across the
    // await would block every other ws-method call.
    let sink = ws
        .lock()
        .await
        .as_ref()
        .map(|backend| Arc::clone(&backend.sink))
        .ok_or(WsError::NotConnected)?;
    Ok(sink.lock().await.send(outgoing).await?)
}

/// The next message the hub sent on the websocket open in `ws`, waiting up to `timeout_ms`.
///
/// `None` when none arrived in that time.
///
/// # Errors
///
/// When the websocket is not open.
pub async fn next_message(ws: &Mutex<Option<WsBackend>>, timeout_ms: u32) -> Result<Option<ServerMessage>, WsError> {
    // Same lock-handoff pattern as `send_frame`: grab the inbox Arc, drop the outer lock, then await.
    let inbox = ws
        .lock()
        .await
        .as_ref()
        .map(|backend| Arc::clone(&backend.inbox))
        .ok_or(WsError::NotConnected)?;
    let mut rx = inbox.lock().await;
    let timeout = Duration::from_millis(u64::from(timeout_ms));
    Ok(tokio::time::timeout(timeout, rx.recv()).await.ok().flatten())
}

// The `et:ws-messages/messages` interface only declares types -- no functions. wasmtime-bindgen still requires a `Host`
// impl so the linker has somewhere to anchor the interface. The trait body is empty.
impl crate::bindings::et::ws_messages::messages::Host for HostState {}

type WsSink = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, tungstenite::Message>;

/// Live state for an open websocket connection.
///
/// Owned by `HostState` behind a `Mutex`; replaced on disconnect.
pub struct WsBackend {
    sink: Arc<Mutex<WsSink>>,
    inbox: Arc<Mutex<mpsc::UnboundedReceiver<ServerMessage>>>,
    agent_id: Arc<Mutex<Option<String>>>,
    connection_state: Arc<Mutex<State>>,
    _reader: JoinHandle<()>,
    _pinger: JoinHandle<()>,
}

impl WsBackend {
    /// Open the socket and complete the et-connect handshake.
    ///
    /// Public so `tests/ws_backend.rs` can drive the reader pump and the heartbeat without standing up a wasmtime
    /// store: both live in tasks this constructor spawns, and nothing else reaches them.
    #[expect(
        clippy::too_many_lines,
        reason = "the handshake and the two tasks it spawns share one socket split"
    )]
    pub async fn connect(ws_url: &str, ack_timeout: Option<Duration>) -> Result<Self, WsError> {
        // The shared helper opens the socket and completes the et-connect handshake (with bounded retries), so the
        // agent_id is known the moment it returns -- no polling for ConnectAck afterwards. `ConnectError` cascades to
        // `WsError` via `From` (see host/error.rs).
        let (socket, assigned_id, status) =
            et_ws_runner_common::connect_and_register(ws_url, None, ack_timeout).await?;
        let (sink, mut stream) = socket.split();

        let (tx, rx) = mpsc::unbounded_channel::<ServerMessage>();
        // The handshake consumed the et-connect-ack frame; re-surface it to the guest's `recv()` so guests that read
        // it still see it as the first message. Cannot fail: `rx` (created just above) is still alive, so the channel
        // is open.
        let _seeded = tx.send(ServerMessage::ConnectAck {
            agent_id: assigned_id.clone(),
            status,
        });

        let agent_id = Arc::new(Mutex::new(Some(assigned_id)));
        let connection_state = Arc::new(Mutex::new(State::Connected));

        // Reader pump: convert every subsequent Text/Binary data frame into a `ServerMessage` (foreign frames land in
        // `RelayText`/`RelayBinary`) and forward it to the guest inbox; drop control frames and
        // et-prefixed-but-malformed text with a warn.
        let state_clone = Arc::clone(&connection_state);
        let reader = tokio::spawn(async move {
            while let Some(msg) = stream.next().await {
                let Ok(msg) = msg else {
                    break;
                };
                #[expect(
                    clippy::wildcard_enum_match_arm,
                    reason = "control frames (Ping/Pong/Close/Frame) and any future variants are dropped"
                )]
                let frame = match msg {
                    tungstenite::Message::Text(text) => Frame::Text(text.as_str().to_owned()),
                    tungstenite::Message::Binary(bytes) => Frame::Binary(bytes.to_vec()),
                    _ => continue,
                };
                let parsed = match server_message(frame) {
                    Ok(msg) => msg,
                    Err(err) => {
                        warn!(error = %err, "dropping et-* frame with decode error");
                        continue;
                    }
                };
                if tx.send(parsed).is_err() {
                    break;
                }
            }
            *state_clone.lock().await = State::Closed;
        });

        let sink_arc = Arc::new(Mutex::new(sink));

        // Heartbeat: server `last_activity` only bumps on inbound frames, so a guest that does multi-second compute
        // between `connect()` and its first send would otherwise trip the 15s idle close. The server's `handle_inbound`
        // counts Ping as activity.
        let pinger_sink = Arc::clone(&sink_arc);
        let pinger_state = Arc::clone(&connection_state);
        let pinger = tokio::spawn(async move {
            // 5s heartbeat (vs the server's 15s idle timeout); first immediate tick already consumed so we don't ping
            // before the handshake.
            let mut interval = et_ws_runner_common::heartbeat_interval().await;
            loop {
                let _: tokio::time::Instant = interval.tick().await;
                if !matches!(*pinger_state.lock().await, State::Connecting | State::Connected) {
                    break;
                }
                let mut guard = pinger_sink.lock().await;
                if guard
                    .send(tungstenite::Message::Ping(tungstenite::Bytes::new()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        Ok(Self {
            sink: sink_arc,
            inbox: Arc::new(Mutex::new(rx)),
            agent_id,
            connection_state,
            _reader: reader,
            _pinger: pinger,
        })
    }

    /// The connection's state as the reader pump last left it.
    pub async fn current_state(&self) -> State {
        *self.connection_state.lock().await
    }

    /// Take the next inbound message, or `None` if none arrives within `timeout`.
    ///
    /// The guest-facing `recv` converts to WIT types on the way out; this is the same drain without that step, so a
    /// caller sees the `ServerMessage` the reader pump actually produced.
    pub async fn next_message(&self, timeout: Duration) -> Option<ServerMessage> {
        let mut rx = self.inbox.lock().await;
        tokio::time::timeout(timeout, rx.recv()).await.ok().flatten()
    }

    async fn current_agent_id(&self) -> String {
        self.agent_id.lock().await.clone().unwrap_or_default()
    }
}

impl Host for HostState {
    async fn connect(&mut self) -> Result<(), WsError> {
        {
            let slot = self.ws.lock().await;
            if slot.is_some() {
                return Err(WsError::AlreadyConnected);
            }
        }
        // `connect_and_register` already completed the handshake, so the backend comes back `Connected` with its
        // agent_id set -- guests can call `agent_id()` immediately, no poll-wait needed.
        let backend = WsBackend::connect(&self.ws_url, self.connect_ack_timeout).await?;
        {
            let mut slot = self.ws.lock().await;
            *slot = Some(backend);
        }
        Ok(())
    }

    async fn get_state(&mut self) -> State {
        let slot = self.ws.lock().await;
        match slot.as_ref() {
            Some(bridge) => bridge.current_state().await,
            None => State::Closed,
        }
    }

    async fn agent_id(&mut self) -> String {
        let slot = self.ws.lock().await;
        match slot.as_ref() {
            Some(bridge) => bridge.current_agent_id().await,
            None => String::default(),
        }
    }

    /// Send the guest's `message` as the frame it goes out as.
    async fn send(&mut self, message: WitClientMessage) -> Result<(), WsError> {
        send_frame(&self.ws, client_frame(message)?).await
    }

    /// The next message, waiting up to `timeout_ms`, in the world's types.
    async fn recv(&mut self, timeout_ms: u32) -> Result<Option<WitServerMessage>, WsError> {
        match next_message(&self.ws, timeout_ms).await? {
            Some(message) => Ok(Some(server_message_to_wit(message)?)),
            None => Ok(None),
        }
    }

    async fn disconnect(&mut self) {
        let mut slot = self.ws.lock().await;
        if let Some(backend) = slot.as_ref() {
            *backend.connection_state.lock().await = State::Closing;
            let _closed: Result<(), _> = backend.sink.lock().await.close().await;
        }
        *slot = None;
    }
}
