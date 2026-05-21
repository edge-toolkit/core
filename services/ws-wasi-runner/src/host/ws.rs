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
use futures_util::SinkExt;
use futures_util::stream::{SplitSink, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite};

use crate::HostState;
use crate::bindings::et::ws_wasi::ws::{Host, State, WsError};
use crate::host::{WsProtocolErrExt, WsTransportErrExt};

type WsSink = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, tungstenite::Message>;

/// Live state for an open websocket connection. Owned by `HostState` behind a
/// `Mutex`; replaced on disconnect.
pub struct WsBackend {
    sink: Arc<Mutex<WsSink>>,
    inbox: Arc<Mutex<mpsc::UnboundedReceiver<String>>>,
    agent_id: Arc<Mutex<Option<String>>>,
    connection_state: Arc<Mutex<State>>,
    _reader: JoinHandle<()>,
}

impl WsBackend {
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
        let agent_id_clone = agent_id.clone();
        let state_clone = connection_state.clone();
        let reader = tokio::spawn(async move {
            while let Some(msg) = stream.next().await {
                let Ok(msg) = msg else {
                    break;
                };
                let tungstenite::Message::Text(text) = msg else {
                    continue;
                };
                let text = text.to_string();
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

        Ok(Self {
            sink: Arc::new(Mutex::new(sink)),
            inbox: Arc::new(Mutex::new(rx)),
            agent_id,
            connection_state,
            _reader: reader,
        })
    }

    async fn send_text(&self, text: String) -> Result<(), WsError> {
        let mut sink = self.sink.lock().await;
        sink.send(tungstenite::Message::text(text))
            .await
            .ws_transport("send text")
    }

    async fn current_state(&self) -> State {
        *self.connection_state.lock().await
    }

    async fn current_agent_id(&self) -> String {
        self.agent_id.lock().await.clone().unwrap_or_default()
    }

    async fn recv(&self, timeout_ms: u32) -> Result<Option<String>, WsError> {
        let mut inbox = self.inbox.lock().await;
        match tokio::time::timeout(Duration::from_millis(timeout_ms as u64), inbox.recv()).await {
            Ok(Some(text)) => Ok(Some(text)),
            Ok(None) => Err(WsError::InboxClosed),
            Err(_) => Ok(None),
        }
    }
}

impl Host for HostState {
    async fn connect(&mut self) -> Result<(), WsError> {
        let mut slot = self.ws.lock().await;
        if slot.is_some() {
            return Err(WsError::AlreadyConnected);
        }
        let backend = WsBackend::connect(&self.ws_url).await?;
        // Wait briefly for ConnectAck before returning, so guests can call
        // agent_id() right after connect() and get a value.
        for _ in 0..50 {
            if matches!(backend.current_state().await, State::Connected) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        *slot = Some(backend);
        Ok(())
    }

    async fn get_state(&mut self) -> State {
        let slot = self.ws.lock().await;
        match slot.as_ref() {
            Some(b) => b.current_state().await,
            None => State::Closed,
        }
    }

    async fn agent_id(&mut self) -> String {
        let slot = self.ws.lock().await;
        match slot.as_ref() {
            Some(b) => b.current_agent_id().await,
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
        backend.send_text(text).await
    }

    async fn recv(&mut self, timeout_ms: u32) -> Result<Option<String>, WsError> {
        let slot = self.ws.lock().await;
        let Some(backend) = slot.as_ref() else {
            return Err(WsError::NotConnected);
        };
        backend.recv(timeout_ms).await
    }

    async fn disconnect(&mut self) {
        let mut slot = self.ws.lock().await;
        if let Some(backend) = slot.as_ref() {
            *backend.connection_state.lock().await = State::Closing;
            let _ = backend.sink.lock().await.close().await;
        }
        *slot = None;
    }
}
