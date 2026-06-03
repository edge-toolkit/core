//! Implements `et:ws-wasi/ws` using `tokio-tungstenite`.
//!
//! On `connect`, we open a websocket, send `WsMessage::Connect { agent_id: None }`,
//! and spawn a task that pumps inbound text messages into a channel. Inbound
//! `connect_ack` messages capture our assigned `agent_id`.
//!
//! `send-event` builds the same `WsMessage::ClientEvent` JSON shape the browser
//! `et-ws-wasm-agent` uses, so the server treats both client kinds identically.

use std::sync::Arc;
use std::time::Duration;

use edge_toolkit::ws::WsMessage;
use futures_util::SinkExt as _;
use futures_util::stream::{SplitSink, StreamExt as _};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite};

use crate::HostState;
use crate::bindings::et::ws_wasi::ws::{Host, State, WsError};
use crate::host::{WsProtocolErrExt as _, WsTransportErrExt as _};

type WsSink = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, tungstenite::Message>;

/// How often the heartbeat task pings the server. Server-side
/// `CONNECTION_TIMEOUT` (services/ws/src/lib.rs:18) is 15 s; pinging at 5 s
/// gives 3x headroom so a slow runner (CI ARM, debug build, large model)
/// still keeps the connection alive across long compute gaps between
/// `connect()` and the first `ClientEvent` the guest sends.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

/// Live state for an open websocket connection. Owned by `HostState` behind a
/// `Mutex`; replaced on disconnect.
pub struct WsBackend {
    sink: Arc<Mutex<WsSink>>,
    inbox: Arc<Mutex<mpsc::UnboundedReceiver<String>>>,
    agent_id: Arc<Mutex<Option<String>>>,
    connection_state: Arc<Mutex<State>>,
    _reader: JoinHandle<()>,
    _pinger: JoinHandle<()>,
}

impl WsBackend {
    #[expect(
        clippy::single_call_fn,
        reason = "inherent constructor; used once by <HostState as Host>::connect"
    )]
    async fn connect(ws_url: &str) -> Result<Self, WsError> {
        let (stream, _) = tokio_tungstenite::connect_async(ws_url)
            .await
            .ws_transport(&format!("ws connect {ws_url}"))?;
        let (mut sink, mut stream) = stream.split();

        // Drive the registration handshake immediately so the agent_id is
        // known by the time `connect()` returns.
        let connect_msg =
            serde_json::to_string(&WsMessage::Connect { agent_id: None }).ws_protocol("serialize connect")?;
        sink.send(tungstenite::Message::text(connect_msg))
            .await
            .ws_transport("send connect")?;

        let (tx, rx) = mpsc::unbounded_channel::<String>();
        let agent_id = Arc::new(Mutex::new(None));
        let connection_state = Arc::new(Mutex::new(State::Connecting));

        // Reader pump: route ConnectAck into `agent_id` + `connection_state`,
        // forward all other text messages to the guest via `inbox`.
        let agent_id_clone = Arc::clone(&agent_id);
        let state_clone = Arc::clone(&connection_state);
        let reader = tokio::spawn(async move {
            while let Some(msg) = stream.next().await {
                let Ok(msg) = msg else {
                    break;
                };
                let tungstenite::Message::Text(text) = msg else {
                    continue;
                };
                let text: String = text.as_str().to_owned();
                if let Ok(parsed) = serde_json::from_str::<WsMessage>(&text)
                    && let WsMessage::ConnectAck { agent_id, .. } = &parsed
                {
                    *agent_id_clone.lock().await = Some(agent_id.clone());
                    *state_clone.lock().await = State::Connected;
                }
                if tx.send(text).is_err() {
                    break;
                }
            }
            *state_clone.lock().await = State::Closed;
        });

        let sink_arc = Arc::new(Mutex::new(sink));

        // Heartbeat: server `last_activity` only bumps on inbound frames, so
        // a guest that does multi-second compute between `connect()` and
        // its first send would otherwise trip the 15s idle close. The
        // server's `handle_inbound` counts Ping as activity.
        let pinger_sink = Arc::clone(&sink_arc);
        let pinger_state = Arc::clone(&connection_state);
        let pinger = tokio::spawn(async move {
            let mut interval = tokio::time::interval(HEARTBEAT_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // First tick fires immediately; skip it so we don't ping before
            // the connect handshake even sees a Connected state.
            let _: tokio::time::Instant = interval.tick().await;
            loop {
                let _: tokio::time::Instant = interval.tick().await;
                if !matches!(*pinger_state.lock().await, State::Connecting | State::Connected) {
                    break;
                }
                let mut guard = pinger_sink.lock().await;
                if guard.send(tungstenite::Message::Ping(Vec::new())).await.is_err() {
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

    async fn current_state(&self) -> State {
        *self.connection_state.lock().await
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
        let backend = WsBackend::connect(&self.ws_url).await?;
        // Wait briefly for ConnectAck before returning, so guests can call
        // agent_id() right after connect() and get a value.
        for _ in 0_u32..50 {
            if matches!(backend.current_state().await, State::Connected) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
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
            None => String::new(),
        }
    }

    async fn send_event(&mut self, category: String, kind: String, body_json: String) -> Result<(), WsError> {
        let body: serde_json::Value = serde_json::from_str(&body_json).ws_protocol("body-json is not valid JSON")?;
        let payload = serde_json::to_string(&WsMessage::ClientEvent {
            capability: category,
            action: kind,
            details: body,
        })
        .ws_protocol("serialize client_event")?;
        self.send_text(payload).await
    }

    async fn send_text(&mut self, text: String) -> Result<(), WsError> {
        let slot = self.ws.lock().await;
        let Some(backend) = slot.as_ref() else {
            return Err(WsError::NotConnected);
        };
        let sink = Arc::clone(&backend.sink);
        drop(slot);
        let mut sink_guard = sink.lock().await;
        sink_guard
            .send(tungstenite::Message::text(text))
            .await
            .ws_transport("send text")
    }

    async fn recv(&mut self, timeout_ms: u32) -> Result<Option<String>, WsError> {
        let slot = self.ws.lock().await;
        let Some(backend) = slot.as_ref() else {
            return Err(WsError::NotConnected);
        };
        let inbox = Arc::clone(&backend.inbox);
        drop(slot);
        let mut inbox_guard = inbox.lock().await;
        match tokio::time::timeout(Duration::from_millis(u64::from(timeout_ms)), inbox_guard.recv()).await {
            Ok(Some(text)) => Ok(Some(text)),
            Ok(None) => Err(WsError::InboxClosed),
            Err(_unused) => Ok(None),
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
