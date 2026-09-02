//! Implements `et:ws-wasi/ws` using `tokio-tungstenite`.
//!
//! On `connect`, we open a websocket, send `ClientMessage::Connect { agent_id: None }`,
//! and spawn a task that pumps inbound text messages into a channel. Inbound
//! `connect_ack` messages capture our assigned `agent_id`.
//!
//! Wire messages cross the WIT boundary as typed `et:ws-messages/messages.ws-message`
//! values. The host converts them to/from `edge_toolkit::ws::{ClientMessage, ServerMessage}` and serialises
//! to JSON for the actual websocket frame. Guests no longer hand-craft JSON.

use std::sync::Arc;
use std::time::Duration;

use edge_toolkit::ws::{
    AgentConnectionState as EtAgentConnectionState, AgentSummary as EtAgentSummary, ClientMessage,
    ConnectStatus as EtConnectStatus, MessageDeliveryStatus as EtMessageDeliveryStatus, MessageScope as EtMessageScope,
    ServerMessage,
};
use futures_util::SinkExt as _;
use futures_util::stream::{SplitSink, StreamExt as _};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite};

use crate::HostState;
use crate::bindings::et::ws_messages::messages::{
    AgentConnectionState as WitAgentConnectionState, AgentSummary as WitAgentSummary,
    ClientMessage as WitClientMessage, ConnectAckPayload, ConnectStatus as WitConnectStatus, InvalidPayload,
    ListAgentsResponsePayload, MessageDeliveryStatus as WitMessageDeliveryStatus, MessageScope as WitMessageScope,
    MessageStatusPayload, RelayBinaryPayload, RelayTextPayload, ResponsePayload, ServerMessage as WitServerMessage,
};
use crate::bindings::et::ws_wasi::ws::{Host, State};

// The `et:ws-messages/messages` interface only declares types -- no functions.
// wasmtime-bindgen still requires a `Host` impl so the linker has somewhere
// to anchor the interface. The trait body is empty.
impl crate::bindings::et::ws_messages::messages::Host for HostState {}

type WsSink = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, tungstenite::Message>;

use crate::bindings::et::ws_wasi::ws::WsError;

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
    #[expect(
        clippy::single_call_fn,
        reason = "inherent constructor; used once by <HostState as Host>::connect"
    )]
    async fn connect(ws_url: &str, ack_timeout: Option<Duration>) -> Result<Self, WsError> {
        // The shared helper opens the socket and completes the et-connect
        // handshake (with bounded retries), so the agent_id is known the moment
        // it returns -- no polling for ConnectAck afterwards.
        // `ConnectError` cascades to `WsError` via `From` (see host/error.rs).
        let (socket, assigned_id, status) =
            et_ws_runner_common::connect_and_register(ws_url, None, ack_timeout).await?;
        let (sink, mut stream) = socket.split();

        let (tx, rx) = mpsc::unbounded_channel::<ServerMessage>();
        // The handshake consumed the et-connect-ack frame; re-surface it to the
        // guest's `recv()` so guests that read it still see it as the first message.
        // Cannot fail: `rx` (created just above) is still alive, so the channel is open.
        let _seeded = tx.send(ServerMessage::ConnectAck {
            agent_id: assigned_id.clone(),
            status,
        });

        let agent_id = Arc::new(Mutex::new(Some(assigned_id)));
        let connection_state = Arc::new(Mutex::new(State::Connected));

        // Reader pump: convert every subsequent Text/Binary data frame into a
        // `ServerMessage` via `ServerMessage::from_*_frame` (foreign frames land
        // in `RelayText`/`RelayBinary`) and forward it to the guest inbox; drop
        // control frames and et-prefixed-but-malformed text with a warn.
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
                let parsed = match msg {
                    tungstenite::Message::Text(text) => match ServerMessage::from_text_frame(text.as_str()) {
                        Ok(msg) => msg,
                        Err(err) => {
                            tracing::warn!(error = %err, "dropping et-* frame with decode error");
                            continue;
                        }
                    },
                    tungstenite::Message::Binary(bytes) => ServerMessage::from_binary_frame(bytes.clone()),
                    _ => continue,
                };
                if tx.send(parsed).is_err() {
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
            // 5s heartbeat (vs the server's 15s idle timeout); first immediate
            // tick already consumed so we don't ping before the handshake.
            let mut interval = et_ws_runner_common::heartbeat_interval().await;
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
        // `connect_and_register` already completed the handshake, so the
        // backend comes back `Connected` with its agent_id set -- guests can
        // call `agent_id()` immediately, no poll-wait needed.
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

    async fn send(&mut self, message: WitClientMessage) -> Result<(), WsError> {
        let et_message = wit_to_client_message(message)?;
        // Relay variants travel as raw text / binary frames (no JSON
        // envelope) -- that's what the ws-server hub forwards verbatim
        // to other agents. Typed variants serialise through standard
        // tagged JSON.
        #[expect(
            clippy::wildcard_enum_match_arm,
            reason = "every non-relay ClientMessage variant takes the standard tagged-JSON path"
        )]
        let outgoing = match et_message {
            ClientMessage::RelayText { content } => tungstenite::Message::text(content),
            ClientMessage::RelayBinary { content } => tungstenite::Message::binary(content),
            typed => {
                let payload = serde_json::to_string(&typed)?;
                tungstenite::Message::text(payload)
            }
        };
        // Clone the sink Arc and release the outer lock before awaiting the
        // inner send -- keeping `self.ws` locked across the await would block
        // every other ws-method call.
        let sink = self
            .ws
            .lock()
            .await
            .as_ref()
            .map(|backend| Arc::clone(&backend.sink))
            .ok_or(WsError::NotConnected)?;
        Ok(sink.lock().await.send(outgoing).await?)
    }

    async fn recv(&mut self, timeout_ms: u32) -> Result<Option<WitServerMessage>, WsError> {
        // Same lock-handoff pattern as `send`: grab the inbox Arc, drop the
        // outer lock, then await.
        let inbox = self
            .ws
            .lock()
            .await
            .as_ref()
            .map(|backend| Arc::clone(&backend.inbox))
            .ok_or(WsError::NotConnected)?;
        let parsed = {
            let mut rx = inbox.lock().await;
            match tokio::time::timeout(Duration::from_millis(u64::from(timeout_ms)), rx.recv()).await {
                Ok(Some(value)) => value,
                Ok(None) | Err(_) => return Ok(None),
            }
        };
        Ok(Some(server_message_to_wit(parsed)?))
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

/// Convert a guest-emitted WIT `client-message` into the canonical Rust `ClientMessage`.
///
/// Opaque JSON fields (sent as `string` over WIT) are parsed here so the host always works with
/// `serde_json::Value` payloads.
#[expect(
    clippy::single_call_fn,
    reason = "named converter; used once by <HostState as Host>::send"
)]
fn wit_to_client_message(msg: WitClientMessage) -> Result<ClientMessage, WsError> {
    let parse_value = |raw: String| -> Result<serde_json::Value, WsError> {
        let mut deserializer = serde_json::Deserializer::from_str(&raw);
        Ok(serde_path_to_error::deserialize(&mut deserializer)?)
    };
    Ok(match msg {
        WitClientMessage::Connect(payload) => ClientMessage::Connect {
            agent_id: payload.agent_id,
        },
        WitClientMessage::Alive(payload) => ClientMessage::Alive {
            timestamp: payload.timestamp,
        },
        WitClientMessage::ListAgents => ClientMessage::ListAgents,
        WitClientMessage::SendAgentMessage(payload) => ClientMessage::SendAgentMessage {
            to_agent_id: payload.to_agent_id,
            message: parse_value(payload.message)?,
        },
        WitClientMessage::BroadcastMessage(payload) => ClientMessage::BroadcastMessage {
            message: parse_value(payload.message)?,
        },
        WitClientMessage::MessageAck(payload) => ClientMessage::MessageAck {
            message_id: payload.message_id,
        },
        WitClientMessage::ClientEvent(payload) => ClientMessage::ClientEvent {
            capability: payload.capability,
            action: payload.action,
            details: parse_value(payload.details)?,
        },
        WitClientMessage::RelayText(payload) => ClientMessage::RelayText {
            content: payload.content,
        },
        WitClientMessage::RelayBinary(payload) => ClientMessage::RelayBinary {
            content: payload.content,
        },
    })
}

/// Convert a wire `ServerMessage` (inbound) into the WIT `server-message` the guest sees on `recv`.
///
/// Re-serialises opaque JSON payloads so they cross the WIT boundary as strings.
#[expect(
    clippy::single_call_fn,
    reason = "named converter; used once by <HostState as Host>::recv"
)]
fn server_message_to_wit(msg: ServerMessage) -> Result<WitServerMessage, WsError> {
    let serialize = |value: serde_json::Value| serde_json::to_string(&value);
    Ok(match msg {
        ServerMessage::ConnectAck { agent_id, status } => WitServerMessage::ConnectAck(ConnectAckPayload {
            agent_id,
            status: et_connect_status(status),
        }),
        ServerMessage::ListAgentsResponse { agents } => {
            WitServerMessage::ListAgentsResponse(ListAgentsResponsePayload {
                agents: agents.into_iter().map(et_agent_summary).collect(),
            })
        }
        ServerMessage::AgentMessage {
            message_id,
            from_agent_id,
            scope,
            server_received_at,
            message,
        } => WitServerMessage::AgentMessage(crate::bindings::et::ws_messages::messages::AgentMessagePayload {
            message_id,
            from_agent_id,
            scope: et_message_scope(scope),
            server_received_at,
            message: serialize(message)?,
        }),
        ServerMessage::MessageStatus {
            message_id,
            status,
            detail,
        } => WitServerMessage::MessageStatus(MessageStatusPayload {
            message_id,
            status: et_delivery_status(status),
            detail,
        }),
        ServerMessage::Invalid { message_id, detail } => {
            WitServerMessage::Invalid(InvalidPayload { detail, message_id })
        }
        ServerMessage::Response { message } => WitServerMessage::Response(ResponsePayload { message }),
        ServerMessage::RelayText { content } => WitServerMessage::RelayText(RelayTextPayload { content }),
        ServerMessage::RelayBinary { content } => WitServerMessage::RelayBinary(RelayBinaryPayload { content }),
    })
}

// edge-toolkit-to-WIT enum mirrors. Only the `et_*` direction is used:
// the host never deserialises a WIT-side client message back into an
// edge-toolkit type (that would mean reading something the *server*
// sent through the *guest's* outbound API, which never happens), so the
// reverse-direction helpers from the pre-split file are gone.
#[expect(
    clippy::single_call_fn,
    clippy::needless_pass_by_value,
    reason = "WIT/edge-toolkit enum bridge; Copy enum, by value keeps call sites uniform"
)]
const fn et_connect_status(value: EtConnectStatus) -> WitConnectStatus {
    match value {
        EtConnectStatus::Assigned => WitConnectStatus::Assigned,
        EtConnectStatus::Reconnected => WitConnectStatus::Reconnected,
    }
}

#[expect(
    clippy::single_call_fn,
    clippy::needless_pass_by_value,
    reason = "WIT/edge-toolkit enum bridge; Copy enum, by value keeps call sites uniform"
)]
const fn et_message_scope(value: EtMessageScope) -> WitMessageScope {
    match value {
        EtMessageScope::Direct => WitMessageScope::Direct,
        EtMessageScope::Broadcast => WitMessageScope::Broadcast,
    }
}

#[expect(
    clippy::single_call_fn,
    clippy::needless_pass_by_value,
    reason = "WIT/edge-toolkit enum bridge; Copy enum, by value keeps call sites uniform"
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
    clippy::needless_pass_by_value,
    reason = "WIT/edge-toolkit enum bridge; Copy enum, by value keeps call sites uniform"
)]
const fn et_agent_connection_state(value: EtAgentConnectionState) -> WitAgentConnectionState {
    match value {
        EtAgentConnectionState::Connected => WitAgentConnectionState::Connected,
        EtAgentConnectionState::Disconnected => WitAgentConnectionState::Disconnected,
    }
}

#[expect(
    clippy::single_call_fn,
    reason = "summary converter; one call site in server_message_to_wit"
)]
fn et_agent_summary(value: EtAgentSummary) -> WitAgentSummary {
    WitAgentSummary {
        agent_id: value.agent_id,
        state: et_agent_connection_state(value.state),
        last_known_ip: value.last_known_ip,
    }
}
