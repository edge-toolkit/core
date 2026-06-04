//! Implements `et:ws-wasi/ws` using `tokio-tungstenite`.
//!
//! On `connect`, we open a websocket, send `WsMessage::Connect { agent_id: None }`,
//! and spawn a task that pumps inbound text messages into a channel. Inbound
//! `connect_ack` messages capture our assigned `agent_id`.
//!
//! Wire messages cross the WIT boundary as typed `et:ws-messages/messages.ws-message`
//! values. The host converts them to/from `edge_toolkit::ws::WsMessage` and serialises
//! to JSON for the actual websocket frame. Guests no longer hand-craft JSON.

use std::sync::Arc;
use std::time::Duration;

use edge_toolkit::ws::{
    AgentConnectionState as EtAgentConnectionState, AgentSummary as EtAgentSummary, ConnectStatus as EtConnectStatus,
    MessageDeliveryStatus as EtMessageDeliveryStatus, MessageScope as EtMessageScope, WsMessage,
};
use futures_util::SinkExt as _;
use futures_util::stream::{SplitSink, StreamExt as _};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite};

use crate::HostState;
use crate::bindings::et::ws_messages::messages::{
    AgentConnectionState as WitAgentConnectionState, AgentSummary as WitAgentSummary, AlivePayload,
    BroadcastMessagePayload, ClientEventPayload, ConnectAckPayload, ConnectPayload, ConnectStatus as WitConnectStatus,
    InvalidPayload, ListAgentsResponsePayload, MessageAckPayload, MessageDeliveryStatus as WitMessageDeliveryStatus,
    MessageScope as WitMessageScope, MessageStatusPayload, ResponsePayload, SendAgentMessagePayload,
    WsMessage as WitWsMessage,
};
use crate::bindings::et::ws_wasi::ws::{Host, State};

// The `et:ws-messages/messages` interface only declares types — no functions.
// wasmtime-bindgen still requires a `Host` impl so the linker has somewhere
// to anchor the interface. The trait body is empty.
impl crate::bindings::et::ws_messages::messages::Host for HostState {}

type WsSink = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, tungstenite::Message>;

/// How often the heartbeat task pings the server. Server-side
/// `CONNECTION_TIMEOUT` (services/ws/src/lib.rs:18) is 15 s; pinging at 5 s
/// gives 3x headroom so a slow runner (CI ARM, debug build, large model)
/// still keeps the connection alive across long compute gaps between
/// `connect()` and the first `ClientEvent` the guest sends.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

use crate::bindings::et::ws_wasi::ws::WsError;
use crate::host::error::WsProtocolErrExt as _;
use crate::host::error::WsTransportErrExt as _;

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

    async fn send(&mut self, message: WitWsMessage) -> Result<(), WsError> {
        let et_message = wit_to_et(message)?;
        let payload = serde_json::to_string(&et_message).ws_protocol("serialize ws-message")?;
        // Clone the sink Arc and release the outer lock before awaiting the
        // inner send — keeping `self.ws` locked across the await would block
        // every other ws-method call.
        let sink = self
            .ws
            .lock()
            .await
            .as_ref()
            .map(|backend| Arc::clone(&backend.sink))
            .ok_or(WsError::NotConnected)?;
        sink.lock()
            .await
            .send(tungstenite::Message::text(payload))
            .await
            .ws_transport("send text")
    }

    async fn recv(&mut self, timeout_ms: u32) -> Result<Option<WitWsMessage>, WsError> {
        // Same lock-handoff pattern as `send`: grab the inbox Arc, drop the
        // outer lock, then await.
        let inbox = self
            .ws
            .lock()
            .await
            .as_ref()
            .map(|backend| Arc::clone(&backend.inbox))
            .ok_or(WsError::NotConnected)?;
        let text = {
            let mut rx = inbox.lock().await;
            match tokio::time::timeout(Duration::from_millis(u64::from(timeout_ms)), rx.recv()).await {
                Ok(Some(value)) => value,
                Ok(None) | Err(_) => return Ok(None),
            }
        };
        // Unrecognised frames (server's hub-broadcast fallback) and binary
        // frames don't parse as WsMessage. Skip them — guests will see `none`
        // until the next typed frame arrives.
        let Ok(parsed) = serde_json::from_str::<WsMessage>(&text) else {
            return Ok(None);
        };
        Ok(Some(et_to_wit(parsed)?))
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

/// Convert a typed WIT message coming from the guest into the canonical Rust
/// wire-format type. Opaque JSON fields (sent as `string` over WIT) are parsed
/// here so the host always works with `serde_json::Value` payloads.
#[expect(
    clippy::single_call_fn,
    reason = "named converter; pairs with et_to_wit and is used once by <HostState as Host>::send"
)]
fn wit_to_et(msg: WitWsMessage) -> Result<WsMessage, WsError> {
    let parse_value = |raw: String, label: &str| {
        serde_json::from_str::<serde_json::Value>(&raw).ws_protocol(&format!("{label}: opaque JSON payload"))
    };
    Ok(match msg {
        WitWsMessage::Connect(payload) => WsMessage::Connect {
            agent_id: payload.agent_id,
        },
        WitWsMessage::ConnectAck(payload) => WsMessage::ConnectAck {
            agent_id: payload.agent_id,
            status: wit_connect_status(payload.status),
        },
        WitWsMessage::Alive(payload) => WsMessage::Alive {
            timestamp: payload.timestamp,
        },
        WitWsMessage::ListAgents => WsMessage::ListAgents,
        WitWsMessage::ListAgentsResponse(payload) => WsMessage::ListAgentsResponse {
            agents: payload.agents.into_iter().map(wit_agent_summary).collect(),
        },
        WitWsMessage::SendAgentMessage(payload) => WsMessage::SendAgentMessage {
            to_agent_id: payload.to_agent_id,
            message: parse_value(payload.message, "send-agent-message")?,
        },
        WitWsMessage::BroadcastMessage(payload) => WsMessage::BroadcastMessage {
            message: parse_value(payload.message, "broadcast-message")?,
        },
        WitWsMessage::AgentMessage(payload) => WsMessage::AgentMessage {
            message_id: payload.message_id,
            from_agent_id: payload.from_agent_id,
            scope: wit_message_scope(payload.scope),
            server_received_at: payload.server_received_at,
            message: parse_value(payload.message, "agent-message")?,
        },
        WitWsMessage::MessageAck(payload) => WsMessage::MessageAck {
            message_id: payload.message_id,
        },
        WitWsMessage::MessageStatus(payload) => WsMessage::MessageStatus {
            message_id: payload.message_id,
            status: wit_delivery_status(payload.status),
            detail: payload.detail,
        },
        WitWsMessage::Invalid(payload) => WsMessage::Invalid {
            message_id: payload.message_id,
            detail: payload.detail,
        },
        WitWsMessage::ClientEvent(payload) => WsMessage::ClientEvent {
            capability: payload.capability,
            action: payload.action,
            details: parse_value(payload.details, "client-event")?,
        },
        WitWsMessage::Response(payload) => WsMessage::Response {
            message: payload.message,
        },
    })
}

/// Reverse of `wit_to_et` — serialise opaque payloads back to JSON strings for
/// the WIT crossing.
#[expect(
    clippy::single_call_fn,
    reason = "named converter; pairs with wit_to_et and is used once by <HostState as Host>::recv"
)]
fn et_to_wit(msg: WsMessage) -> Result<WitWsMessage, WsError> {
    let serialize = |value: serde_json::Value| serde_json::to_string(&value).ws_protocol("re-serialize opaque payload");
    Ok(match msg {
        WsMessage::Connect { agent_id } => WitWsMessage::Connect(ConnectPayload { agent_id }),
        WsMessage::ConnectAck { agent_id, status } => WitWsMessage::ConnectAck(ConnectAckPayload {
            agent_id,
            status: et_connect_status(status),
        }),
        WsMessage::Alive { timestamp } => WitWsMessage::Alive(AlivePayload { timestamp }),
        WsMessage::ListAgents => WitWsMessage::ListAgents,
        WsMessage::ListAgentsResponse { agents } => WitWsMessage::ListAgentsResponse(ListAgentsResponsePayload {
            agents: agents.into_iter().map(et_agent_summary).collect(),
        }),
        WsMessage::SendAgentMessage { to_agent_id, message } => {
            WitWsMessage::SendAgentMessage(SendAgentMessagePayload {
                to_agent_id,
                message: serialize(message)?,
            })
        }
        WsMessage::BroadcastMessage { message } => WitWsMessage::BroadcastMessage(BroadcastMessagePayload {
            message: serialize(message)?,
        }),
        WsMessage::AgentMessage {
            message_id,
            from_agent_id,
            scope,
            server_received_at,
            message,
        } => WitWsMessage::AgentMessage(crate::bindings::et::ws_messages::messages::AgentMessagePayload {
            message_id,
            from_agent_id,
            scope: et_message_scope(scope),
            server_received_at,
            message: serialize(message)?,
        }),
        WsMessage::MessageAck { message_id } => WitWsMessage::MessageAck(MessageAckPayload { message_id }),
        WsMessage::MessageStatus {
            message_id,
            status,
            detail,
        } => WitWsMessage::MessageStatus(MessageStatusPayload {
            message_id,
            status: et_delivery_status(status),
            detail,
        }),
        WsMessage::Invalid { message_id, detail } => WitWsMessage::Invalid(InvalidPayload { detail, message_id }),
        WsMessage::ClientEvent {
            capability,
            action,
            details,
        } => WitWsMessage::ClientEvent(ClientEventPayload {
            capability,
            action,
            details: serialize(details)?,
        }),
        WsMessage::Response { message } => WitWsMessage::Response(ResponsePayload { message }),
    })
}

// One-line WIT-to-edge-toolkit and back enum mirrors. Each is `Copy` (no payload)
// so by-value is the correct calling convention; each is called from exactly
// one match arm in `wit_to_et` / `et_to_wit`.
#[expect(
    clippy::single_call_fn,
    reason = "WIT/edge-toolkit enum bridge; one call site, named for symmetry with et_connect_status"
)]
const fn wit_connect_status(value: WitConnectStatus) -> EtConnectStatus {
    match value {
        WitConnectStatus::Assigned => EtConnectStatus::Assigned,
        WitConnectStatus::Reconnected => EtConnectStatus::Reconnected,
    }
}

#[expect(
    clippy::single_call_fn,
    clippy::needless_pass_by_value,
    reason = "WIT/edge-toolkit enum bridge; Copy enum, by value matches wit_connect_status' direction"
)]
const fn et_connect_status(value: EtConnectStatus) -> WitConnectStatus {
    match value {
        EtConnectStatus::Assigned => WitConnectStatus::Assigned,
        EtConnectStatus::Reconnected => WitConnectStatus::Reconnected,
    }
}

#[expect(
    clippy::single_call_fn,
    reason = "WIT/edge-toolkit enum bridge; one call site, named for symmetry with et_message_scope"
)]
const fn wit_message_scope(value: WitMessageScope) -> EtMessageScope {
    match value {
        WitMessageScope::Direct => EtMessageScope::Direct,
        WitMessageScope::Broadcast => EtMessageScope::Broadcast,
    }
}

#[expect(
    clippy::single_call_fn,
    clippy::needless_pass_by_value,
    reason = "WIT/edge-toolkit enum bridge; Copy enum, by value matches wit_message_scope' direction"
)]
const fn et_message_scope(value: EtMessageScope) -> WitMessageScope {
    match value {
        EtMessageScope::Direct => WitMessageScope::Direct,
        EtMessageScope::Broadcast => WitMessageScope::Broadcast,
    }
}

#[expect(
    clippy::single_call_fn,
    reason = "WIT/edge-toolkit enum bridge; one call site, named for symmetry with et_delivery_status"
)]
const fn wit_delivery_status(value: WitMessageDeliveryStatus) -> EtMessageDeliveryStatus {
    match value {
        WitMessageDeliveryStatus::Delivered => EtMessageDeliveryStatus::Delivered,
        WitMessageDeliveryStatus::Queued => EtMessageDeliveryStatus::Queued,
        WitMessageDeliveryStatus::Acknowledged => EtMessageDeliveryStatus::Acknowledged,
        WitMessageDeliveryStatus::Broadcast => EtMessageDeliveryStatus::Broadcast,
    }
}

#[expect(
    clippy::single_call_fn,
    clippy::needless_pass_by_value,
    reason = "WIT/edge-toolkit enum bridge; Copy enum, by value matches wit_delivery_status' direction"
)]
const fn et_delivery_status(value: EtMessageDeliveryStatus) -> WitMessageDeliveryStatus {
    match value {
        EtMessageDeliveryStatus::Delivered => WitMessageDeliveryStatus::Delivered,
        EtMessageDeliveryStatus::Queued => WitMessageDeliveryStatus::Queued,
        EtMessageDeliveryStatus::Acknowledged => WitMessageDeliveryStatus::Acknowledged,
        EtMessageDeliveryStatus::Broadcast => WitMessageDeliveryStatus::Broadcast,
    }
}

#[expect(
    clippy::single_call_fn,
    reason = "WIT/edge-toolkit enum bridge; one call site (inside wit_agent_summary)"
)]
const fn wit_agent_connection_state(value: WitAgentConnectionState) -> EtAgentConnectionState {
    match value {
        WitAgentConnectionState::Connected => EtAgentConnectionState::Connected,
        WitAgentConnectionState::Disconnected => EtAgentConnectionState::Disconnected,
    }
}

#[expect(
    clippy::single_call_fn,
    clippy::needless_pass_by_value,
    reason = "WIT/edge-toolkit enum bridge; Copy enum, by value matches wit_agent_connection_state' direction"
)]
const fn et_agent_connection_state(value: EtAgentConnectionState) -> WitAgentConnectionState {
    match value {
        EtAgentConnectionState::Connected => WitAgentConnectionState::Connected,
        EtAgentConnectionState::Disconnected => WitAgentConnectionState::Disconnected,
    }
}

#[expect(clippy::single_call_fn, reason = "summary converter; one call site in wit_to_et")]
fn wit_agent_summary(value: WitAgentSummary) -> EtAgentSummary {
    EtAgentSummary::new(
        value.agent_id,
        wit_agent_connection_state(value.state),
        value.last_known_ip,
    )
}

#[expect(clippy::single_call_fn, reason = "summary converter; one call site in et_to_wit")]
fn et_agent_summary(value: EtAgentSummary) -> WitAgentSummary {
    WitAgentSummary {
        agent_id: value.agent_id,
        state: et_agent_connection_state(value.state),
        last_known_ip: value.last_known_ip,
    }
}
