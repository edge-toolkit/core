#![cfg_attr(doc, doc = include_str!("../README.md"))]
#![cfg_attr(feature = "docs", doc = "## Feature flags")]
#![cfg_attr(feature = "docs", doc = document_features::document_features!())]
#![expect(
    clippy::single_call_fn,
    unused_results,
    reason = "load_/store_ helpers each called once but kept named; Reflect::set's bool result discarded by design"
)]

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use edge_toolkit::ws::{ClientMessage, ConnectStatus, ServerMessage};
use et_web::JsResultExt as _;
use tracing::{error, info, warn};
use wasm_bindgen::prelude::*;
use web_sys::{Event, MessageEvent, WebSocket};

mod dom;
mod session;

pub use self::dom::{append_to_textarea, js_bool_field, js_nested_object, js_number_field, set_textarea_value};
use self::dom::{load_stored_agent_id, store_agent_id};

const MAX_OFFLINE_QUEUE_LEN: usize = 1000;
/// Default cadence for client-side app-level `Alive` messages sent to the websocket server.
/// This should remain comfortably lower than the server's idle connection timeout.
const DEFAULT_ALIVE_INTERVAL_MS: u32 = 5_000;

// Initialize logging for WASM
pub fn init_logging() {
    tracing_wasm::set_as_global_default();
    info!("WebSocket client initialized");
}

#[wasm_bindgen(js_name = initTracing)]
pub fn init_tracing() {
    init_logging();
}

// Connection state
#[expect(
    clippy::exhaustive_enums,
    reason = "ConnectionState enumerates the WebSocket client's lifecycle; downstream code matches exhaustively"
)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
}

// WebSocket client configuration
#[wasm_bindgen]
pub struct WsClientConfig {
    server_url: String,
    alive_interval_ms: u32,
    max_reconnect_attempts: u32,
    initial_reconnect_delay_ms: u32,
    use_retained_agent_id: bool,
}

#[wasm_bindgen]
#[expect(
    clippy::missing_const_for_fn,
    reason = "wasm_bindgen rejects const fns; methods cannot be marked const"
)]
impl WsClientConfig {
    #[must_use]
    #[wasm_bindgen(constructor)]
    pub fn new(server_url: String) -> Self {
        Self {
            server_url,
            alive_interval_ms: DEFAULT_ALIVE_INTERVAL_MS,
            max_reconnect_attempts: 10,
            initial_reconnect_delay_ms: 1000,
            use_retained_agent_id: true,
        }
    }

    #[wasm_bindgen(setter)]
    pub fn set_alive_interval(&mut self, interval_ms: u32) {
        self.alive_interval_ms = interval_ms;
    }

    #[wasm_bindgen(setter)]
    pub fn set_max_reconnect_attempts(&mut self, attempts: u32) {
        self.max_reconnect_attempts = attempts;
    }

    #[wasm_bindgen(setter)]
    pub fn set_initial_reconnect_delay(&mut self, delay_ms: u32) {
        self.initial_reconnect_delay_ms = delay_ms;
    }

    /// Opt out of the shared retained agent id for an ephemeral, per-client identity.
    ///
    /// Every client on an origin shares one retained agent id in localStorage, and the server keeps a single
    /// session per agent id -- so a page's own client and a module's client using the same id steal each
    /// other's registration on every (re)connect. A client that sets this to `false` neither loads nor
    /// stores the retained id: the server assigns it a fresh id and it coexists with the page's client.
    #[wasm_bindgen(setter)]
    pub fn set_use_retained_agent_id(&mut self, use_retained: bool) {
        self.use_retained_agent_id = use_retained;
    }
}

// Inner shared state
struct SharedState {
    socket: Option<WebSocket>,
    state: ConnectionState,
    alive_interval_id: Option<i32>,
    reconnect_timeout_id: Option<i32>,
    offline_queue: VecDeque<String>,
    manual_disconnect: bool,
    reconnect_attempts: u32,
    reconnect_delay_ms: u32,
    on_message_callback: Option<JsValue>,
    on_state_change_callback: Option<JsValue>,
}

// Main WebSocket client
#[wasm_bindgen]
pub struct WsClient {
    config: WsClientConfig,
    agent_id: Rc<RefCell<Option<String>>>,
    shared: Rc<RefCell<SharedState>>,
}

#[wasm_bindgen]
impl WsClient {
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new(config: WsClientConfig) -> Self {
        let agent_id = if config.use_retained_agent_id {
            load_stored_agent_id()
        } else {
            None
        };
        info!("Creating new WebSocket client with retained agent ID: {:?}", agent_id);

        let shared = Rc::new(RefCell::new(SharedState {
            socket: None,
            state: ConnectionState::Disconnected,
            alive_interval_id: None,
            reconnect_timeout_id: None,
            offline_queue: VecDeque::with_capacity(MAX_OFFLINE_QUEUE_LEN),
            manual_disconnect: false,
            reconnect_attempts: 0,
            reconnect_delay_ms: 1000,
            on_message_callback: None,
            on_state_change_callback: None,
        }));

        Self {
            config,
            agent_id: Rc::new(RefCell::new(agent_id)),
            shared,
        }
    }

    /// Connect to the WebSocket server.
    #[wasm_bindgen]
    #[expect(
        clippy::cognitive_complexity,
        clippy::too_many_lines,
        reason = "single-method connect+wire-up; on_message closure dispatches all ServerMessage variants inline"
    )]
    pub fn connect(&mut self) -> Result<(), JsValue> {
        info!("Connecting to WebSocket server: {}", self.config.server_url);

        let _window = web_sys::window().ok_or("No window available")?;
        let socket = WebSocket::new(&self.config.server_url).js_context("Failed to create WebSocket")?;

        // Set binary type to arraybuffer
        socket.set_binary_type(web_sys::BinaryType::Arraybuffer);

        // Store the socket
        {
            let mut state = self.shared.borrow_mut();
            state.socket = Some(socket.clone());
            state.state = ConnectionState::Connecting;
            state.manual_disconnect = false;
        }
        self.notify_state_change();

        // Set up event handlers
        let on_open_box: Box<dyn FnMut(Event)> = Box::new({
            let shared = Rc::clone(&self.shared);
            let initial_delay = self.config.initial_reconnect_delay_ms;
            let cli_ptr = self.clone();
            move |_event: Event| {
                info!("WebSocket connected");
                {
                    let mut state = shared.borrow_mut();
                    state.state = ConnectionState::Connected;
                    state.reconnect_attempts = 0;
                    state.reconnect_delay_ms = initial_delay;
                    if let Some(timeout_id) = state.reconnect_timeout_id.take()
                        && let Some(window) = web_sys::window()
                    {
                        window.clear_timeout_with_handle(timeout_id);
                    }
                }
                cli_ptr.notify_state_change();
                if let Err(error) = cli_ptr.send_connect_message() {
                    error!("Failed to send connect message: {:?}", error);
                }
                cli_ptr.flush_offline_queue();
                cli_ptr.start_alive_interval();
            }
        });
        let on_open = Closure::wrap(on_open_box);

        let on_message_box: Box<dyn FnMut(MessageEvent)> = Box::new({
            let shared = Rc::clone(&self.shared);
            let retained_agent_id = Rc::clone(&self.agent_id);
            let use_retained_agent_id = self.config.use_retained_agent_id;
            move |event: MessageEvent| {
                info!("WebSocket message received");
                if let Some(data) = event.data().as_string() {
                    info!("Received: {}", data);
                    // Try to parse and handle the message
                    if let Ok(msg) = serde_json::from_str::<ServerMessage>(&data) {
                        match msg {
                            ServerMessage::ConnectAck { agent_id, status } => {
                                info!(
                                    "Server connect acknowledgement: agent_id={} status={:?}",
                                    agent_id, status
                                );
                                *retained_agent_id.borrow_mut() = Some(agent_id.clone());
                                // An ephemeral-identity client must not clobber the page's shared retained
                                // id with its own throwaway one.
                                if use_retained_agent_id && let Err(error) = store_agent_id(&agent_id) {
                                    warn!("Failed to persist agent ID: {:?}", error);
                                }
                                match status {
                                    ConnectStatus::Assigned => {
                                        info!("Server assigned a new agent_id");
                                    }
                                    ConnectStatus::Reconnected => {
                                        info!("Server accepted retained agent_id");
                                    }
                                }
                            }
                            ServerMessage::Response { message } => {
                                info!("Server response: {}", message);
                            }
                            ServerMessage::ListAgentsResponse { agents } => {
                                info!("Server returned {} agents", agents.len());
                            }
                            ServerMessage::AgentMessage {
                                message_id,
                                from_agent_id,
                                scope,
                                server_received_at,
                                ..
                            } => {
                                info!(
                                    "Received {:?} agent message {} from {} at {}",
                                    scope, message_id, from_agent_id, server_received_at
                                );
                            }
                            ServerMessage::MessageStatus {
                                message_id,
                                status,
                                detail,
                            } => {
                                info!("Message status update {:?} {:?}: {}", message_id, status, detail);
                            }
                            ServerMessage::Invalid { message_id, detail } => {
                                warn!("Invalid server message {:?}: {}", message_id, detail);
                            }
                            ServerMessage::RelayText { content } => {
                                info!("Server relayed text frame ({} bytes)", content.len());
                            }
                            ServerMessage::RelayBinary { content } => {
                                info!("Server relayed binary frame ({} bytes)", content.len());
                            }
                        }
                    }
                    // Notify callback if set
                    let state = shared.borrow();
                    if let Some(callback) = &state.on_message_callback
                        && let Some(function) = callback.dyn_ref::<js_sys::Function>()
                    {
                        let _called: Result<JsValue, JsValue> =
                            function.call1(&JsValue::NULL, &JsValue::from_str(&data));
                    }
                }
            }
        });
        let on_message = Closure::wrap(on_message_box);

        let on_error_box: Box<dyn FnMut(Event)> = Box::new({
            let cli_ptr = self.clone();
            move |_event: Event| {
                error!("WebSocket error occurred");
                cli_ptr.handle_disconnect();
            }
        });
        let on_error = Closure::wrap(on_error_box);

        let on_close_box: Box<dyn FnMut(Event)> = Box::new({
            let cli_ptr = self.clone();
            move |_event: Event| {
                info!("WebSocket closed");
                cli_ptr.handle_disconnect();
            }
        });
        let on_close = Closure::wrap(on_close_box);

        // Add event listeners
        socket.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        socket.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));

        // Forget the closures to keep them alive
        on_open.forget();
        on_message.forget();
        on_error.forget();
        on_close.forget();

        Ok(())
    }

    /// Disconnect from the WebSocket server.
    #[wasm_bindgen]
    pub fn disconnect(&mut self) {
        info!("Disconnecting WebSocket client");
        self.stop_alive_interval();
        self.cancel_reconnect();
        self.record_offline();
        {
            let mut state = self.shared.borrow_mut();
            state.manual_disconnect = true;
            if let Some(socket) = &state.socket {
                let _closed: Result<(), JsValue> = socket.close();
            }
            state.socket = None;
            state.state = ConnectionState::Disconnected;
        }
        self.notify_state_change();
    }

    /// Send an alive message to the server.
    #[wasm_bindgen]
    pub fn send_alive(&self) -> Result<(), JsValue> {
        let state = self.shared.borrow();
        if state.state != ConnectionState::Connected {
            return Err(JsValue::from_str("Not connected"));
        }

        let timestamp = chrono::Utc::now().to_rfc3339();
        let msg = ClientMessage::Alive { timestamp };

        let json = serde_json::to_string(&msg).js_context("Failed to serialize message")?;

        if let Some(socket) = &state.socket {
            socket.send_with_str(&json).js_context("Failed to send message")?;
            info!("Alive message sent: {}", json);
        }

        Ok(())
    }

    /// Send a custom message to the server.
    #[wasm_bindgen]
    #[expect(
        clippy::cognitive_complexity,
        reason = "the score is info!/warn! expansion; the body branches twice"
    )]
    pub fn send(&self, message: &str) -> Result<(), JsValue> {
        let should_queue = {
            let state = self.shared.borrow();
            state.state != ConnectionState::Connected || state.socket.is_none()
        };

        if should_queue {
            self.enqueue_offline_message(message);
            return Ok(());
        }

        let send_result = {
            let state = self.shared.borrow();
            state
                .socket
                .as_ref()
                .ok_or_else(|| JsValue::from_str("No websocket available"))?
                .send_with_str(message)
        };

        match send_result {
            Ok(()) => {
                info!("Message sent: {}", message);
                Ok(())
            }
            Err(error) => {
                warn!("Send failed while online, queueing message for retry: {:?}", error);
                self.enqueue_offline_message(message);
                Err(JsValue::from_str(&format!(
                    "Failed to send message immediately; queued for retry: {error:?}"
                )))
            }
        }
    }

    /// Get the current connection state.
    #[wasm_bindgen]
    #[must_use]
    pub fn get_state(&self) -> String {
        match self.shared.borrow().state {
            ConnectionState::Disconnected => "disconnected".to_string(),
            ConnectionState::Connecting => "connecting".to_string(),
            ConnectionState::Connected => "connected".to_string(),
            ConnectionState::Reconnecting => "reconnecting".to_string(),
        }
    }

    /// Get the agent ID assigned by the server on connect.
    #[wasm_bindgen]
    #[must_use]
    pub fn get_agent_id(&self) -> String {
        self.agent_id.borrow().clone().unwrap_or_default()
    }

    /// Set callback for message events.
    #[wasm_bindgen]
    pub fn set_on_message(&mut self, callback: JsValue) {
        self.shared.borrow_mut().on_message_callback = Some(callback);
    }

    /// Set callback for state change events.
    #[wasm_bindgen]
    pub fn set_on_state_change(&mut self, callback: JsValue) {
        self.shared.borrow_mut().on_state_change_callback = Some(callback);
    }
}

// Helper function to create a client and connect
#[wasm_bindgen]
pub fn create_and_connect(server_url: String) -> Result<WsClient, JsValue> {
    let config = WsClientConfig::new(server_url);
    let mut client = WsClient::new(config);
    client.connect()?;
    Ok(client)
}

/// Poll `client` until it reports `connected`, for up to ten seconds.
#[expect(
    clippy::future_not_send,
    reason = "awaits et_web::sleep_ms, whose JsFuture is Rc-backed and never Send; single-threaded browser WASM"
)]
pub async fn wait_for_connected(client: &WsClient) -> Result<(), JsValue> {
    for _ in 0_u32..100 {
        if client.get_state() == "connected" {
            return Ok(());
        }
        et_web::sleep_ms(100).await?;
    }

    Err(JsValue::from_str("Timed out waiting for websocket connection"))
}
