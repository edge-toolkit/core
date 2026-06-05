#![expect(
    clippy::single_call_fn,
    reason = "load_/store_ localStorage helpers are a matched group; each is invoked once but kept named for symmetry"
)]
#![expect(
    unused_results,
    reason = "js_sys::Reflect::set's bool result is deliberately discarded for fire-and-forget UI updates"
)]

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use edge_toolkit::ws::{ClientMessage, ConnectStatus, ServerMessage};
use et_web::JsResultExt as _;
use tracing::{error, info, warn};
use wasm_bindgen::prelude::*;
use web_sys::{Event, MessageEvent, WebSocket};

const STORED_AGENT_ID_KEY: &str = "ws_wasm_agent.agent_id";
const STORED_LAST_OFFLINE_AT_KEY: &str = "ws_wasm_agent.last_offline_at";
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
}

#[must_use]
pub fn js_number_field(value: &JsValue, field: &str) -> Option<f64> {
    let field_value = js_sys::Reflect::get(value, &JsValue::from_str(field)).ok()?;
    field_value.as_f64()
}

#[must_use]
pub fn js_bool_field(value: &JsValue, field: &str) -> Option<bool> {
    let field_value = js_sys::Reflect::get(value, &JsValue::from_str(field)).ok()?;
    field_value.as_bool()
}

#[must_use]
pub fn js_nested_object(value: &JsValue, field: &str) -> Option<JsValue> {
    js_sys::Reflect::get(value, &JsValue::from_str(field))
        .ok()
        .filter(|nested| !nested.is_null() && !nested.is_undefined())
}

// WebSocket client configuration
#[wasm_bindgen]
pub struct WsClientConfig {
    server_url: String,
    alive_interval_ms: u32,
    max_reconnect_attempts: u32,
    initial_reconnect_delay_ms: u32,
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
        let agent_id = load_stored_agent_id();
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

    /// Connect to the WebSocket server
    #[wasm_bindgen]
    #[expect(
        clippy::too_many_lines,
        clippy::cognitive_complexity,
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
                                if let Err(error) = store_agent_id(&agent_id) {
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

    /// Disconnect from the WebSocket server
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

    /// Send an alive message to the server
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

    /// Send a custom message to the server
    #[wasm_bindgen]
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

    /// Get the current connection state
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

    /// Set callback for message events
    #[wasm_bindgen]
    pub fn set_on_message(&mut self, callback: JsValue) {
        self.shared.borrow_mut().on_message_callback = Some(callback);
    }

    /// Set callback for state change events
    #[wasm_bindgen]
    pub fn set_on_state_change(&mut self, callback: JsValue) {
        self.shared.borrow_mut().on_state_change_callback = Some(callback);
    }

    // Internal methods

    fn start_alive_interval(&self) {
        self.stop_alive_interval();

        let Some(window) = web_sys::window() else {
            warn!("No window available to start alive interval");
            return;
        };
        let Ok(interval_ms) = i32::try_from(self.config.alive_interval_ms) else {
            warn!(
                "alive_interval_ms ({}) exceeds i32::MAX; skipping interval",
                self.config.alive_interval_ms
            );
            return;
        };

        let interval_closure = self.build_alive_closure();
        self.install_alive_interval(&window, interval_ms, interval_closure);
    }

    fn build_alive_closure(&self) -> Closure<dyn FnMut()> {
        let cli_ptr = self.clone();
        let interval_box: Box<dyn FnMut()> = Box::new(move || {
            if let Err(error) = cli_ptr.send_alive() {
                warn!("Failed to send alive keepalive: {:?}", error);
            }
        });
        Closure::wrap(interval_box)
    }

    fn install_alive_interval(
        &self,
        window: &web_sys::Window,
        interval_ms: i32,
        interval_closure: Closure<dyn FnMut()>,
    ) {
        match window.set_interval_with_callback_and_timeout_and_arguments_0(
            interval_closure.as_ref().unchecked_ref(),
            interval_ms,
        ) {
            Ok(interval_id) => {
                self.shared.borrow_mut().alive_interval_id = Some(interval_id);
                info!("Started alive interval at {}ms", self.config.alive_interval_ms);
                interval_closure.forget();
            }
            Err(error) => {
                warn!("Failed to start alive interval: {:?}", error);
            }
        }
    }

    fn stop_alive_interval(&self) {
        let mut state = self.shared.borrow_mut();
        if let Some(interval_id) = state.alive_interval_id.take() {
            if let Some(window) = web_sys::window() {
                window.clear_interval_with_handle(interval_id);
            }
            info!("Stopped alive interval");
        }
    }

    fn handle_disconnect(&self) {
        self.stop_alive_interval();
        let manual_disconnect = {
            let mut state = self.shared.borrow_mut();
            state.socket = None;
            state.state = ConnectionState::Disconnected;
            state.manual_disconnect
        };
        self.record_offline();
        self.notify_state_change();

        if manual_disconnect {
            info!("Manual websocket disconnect; skipping reconnect");
            return;
        }

        // Attempt reconnection with exponential backoff
        let mut do_reconnect = false;
        let mut next_delay = 0_u32;
        let mut curr_attempt = 0_u32;
        {
            let mut state = self.shared.borrow_mut();
            if state.reconnect_attempts < self.config.max_reconnect_attempts {
                state.state = ConnectionState::Reconnecting;
                next_delay = state.reconnect_delay_ms;
                state.reconnect_delay_ms = state.reconnect_delay_ms.saturating_mul(2).min(30_000);
                state.reconnect_attempts = state.reconnect_attempts.saturating_add(1);
                curr_attempt = state.reconnect_attempts;
                do_reconnect = true;
            }
        }
        if do_reconnect {
            self.notify_state_change();
            info!("Attempting reconnection {} in {}ms", curr_attempt, next_delay);
            let delay_i32 = i32::try_from(next_delay).unwrap_or(i32::MAX);
            self.schedule_reconnect(delay_i32);
        } else {
            error!("Max reconnection attempts reached");
        }
    }

    fn notify_state_change(&self) {
        let state_label = self.get_state();
        let state = self.shared.borrow();
        if let Some(callback) = &state.on_state_change_callback
            && let Some(function) = callback.dyn_ref::<js_sys::Function>()
        {
            let _called: Result<JsValue, JsValue> = function.call1(&JsValue::NULL, &JsValue::from_str(&state_label));
        }
    }

    fn send_connect_message(&self) -> Result<(), JsValue> {
        let state = self.shared.borrow();
        let msg = ClientMessage::Connect {
            agent_id: self.agent_id.borrow().clone(),
        };

        let json = serde_json::to_string(&msg).js_context("Failed to serialize connect message")?;

        if let Some(socket) = &state.socket {
            socket
                .send_with_str(&json)
                .js_context("Failed to send connect message")?;
            info!("Connect message sent: {}", json);
        }

        Ok(())
    }

    fn enqueue_offline_message(&self, message: &str) {
        let mut state = self.shared.borrow_mut();
        if state.offline_queue.len() == MAX_OFFLINE_QUEUE_LEN {
            let _dropped: Option<String> = state.offline_queue.pop_front();
            warn!(
                "Offline websocket queue reached {} messages; dropping oldest entry",
                MAX_OFFLINE_QUEUE_LEN
            );
        }
        state.offline_queue.push_back(message.to_string());
        info!(
            "Queued websocket message while offline (queue_len={}): {}",
            state.offline_queue.len(),
            message
        );
    }

    fn flush_offline_queue(&self) {
        loop {
            let next_message = {
                let mut state = self.shared.borrow_mut();
                if state.state != ConnectionState::Connected || state.socket.is_none() {
                    return;
                }
                state.offline_queue.pop_front()
            };

            let Some(message) = next_message else {
                return;
            };

            let send_result: Result<(), JsValue> = {
                let state = self.shared.borrow();
                #[expect(
                    clippy::option_if_let_else,
                    reason = "map_or_else inverts reading order (None-branch first) for two Result-returning closures"
                )]
                match state.socket.as_ref() {
                    Some(socket) => socket
                        .send_with_str(&message)
                        .js_context("Failed to flush queued message"),
                    None => Err(JsValue::from_str("No websocket available")),
                }
            };

            if let Err(error) = send_result {
                warn!("Failed to flush queued websocket message; re-queueing: {:?}", error);
                let mut state = self.shared.borrow_mut();
                state.offline_queue.push_front(message);
                return;
            }

            info!("Flushed queued websocket message: {}", message);
        }
    }

    #[expect(
        clippy::unused_self,
        reason = "kept on &self to mirror the other client lifecycle methods; recorded state lives in localStorage"
    )]
    fn record_offline(&self) {
        let timestamp = chrono::Utc::now().to_rfc3339();
        match store_last_offline_at(&timestamp) {
            Ok(()) => info!("Recorded websocket offline transition at {}", timestamp),
            Err(error) => warn!("Failed to record websocket offline transition: {:?}", error),
        }
    }

    fn schedule_reconnect(&self, delay_ms: i32) {
        self.cancel_reconnect();

        let Some(window) = web_sys::window() else {
            warn!("No window available to schedule reconnect");
            return;
        };

        let mut cli_ptr = self.clone();
        let reconnect_box: Box<dyn FnOnce()> = Box::new(move || {
            if let Err(error) = cli_ptr.connect() {
                error!("Reconnect attempt failed: {:?}", error);
            }
        });
        let reconnect_closure = Closure::once(reconnect_box);

        match window
            .set_timeout_with_callback_and_timeout_and_arguments_0(reconnect_closure.as_ref().unchecked_ref(), delay_ms)
        {
            Ok(timeout_id) => {
                self.shared.borrow_mut().reconnect_timeout_id = Some(timeout_id);
                reconnect_closure.forget();
            }
            Err(error) => {
                warn!("Failed to schedule reconnect: {:?}", error);
            }
        }
    }

    fn cancel_reconnect(&self) {
        let mut state = self.shared.borrow_mut();
        if let Some(timeout_id) = state.reconnect_timeout_id.take()
            && let Some(window) = web_sys::window()
        {
            window.clear_timeout_with_handle(timeout_id);
        }
    }
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "second block holds methods that wasm_bindgen can't export (generics, serde_json::Value)"
)]
impl WsClient {
    pub fn request_list_agents(&self) -> Result<(), JsValue> {
        let payload =
            serde_json::to_string(&ClientMessage::ListAgents).js_context("Failed to serialize list_agents")?;
        self.send(&payload)
    }

    pub fn broadcast_message(&self, message: serde_json::Value) -> Result<(), JsValue> {
        let payload = serde_json::to_string(&ClientMessage::BroadcastMessage { message })
            .js_context("Failed to serialize broadcast message")?;
        self.send(&payload)
    }

    pub fn send_agent_message<T: Into<String>>(
        &self,
        to_agent_id: T,
        message: serde_json::Value,
    ) -> Result<(), JsValue> {
        let payload = serde_json::to_string(&ClientMessage::SendAgentMessage {
            to_agent_id: to_agent_id.into(),
            message,
        })
        .js_context("Failed to serialize direct message")?;
        self.send(&payload)
    }

    pub fn send_client_event<C: Into<String>, A: Into<String>>(
        &self,
        capability: C,
        action: A,
        details: serde_json::Value,
    ) -> Result<(), JsValue> {
        let message = ClientMessage::ClientEvent {
            capability: capability.into(),
            action: action.into(),
            details,
        };
        let payload = serde_json::to_string(&message).js_context("Failed to serialize client event")?;
        self.send(&payload)
    }
}

// Implement Clone for WsClient (required for closures)
impl Clone for WsClient {
    fn clone(&self) -> Self {
        Self {
            config: WsClientConfig {
                server_url: self.config.server_url.clone(),
                alive_interval_ms: self.config.alive_interval_ms,
                max_reconnect_attempts: self.config.max_reconnect_attempts,
                initial_reconnect_delay_ms: self.config.initial_reconnect_delay_ms,
            },
            agent_id: Rc::clone(&self.agent_id),
            shared: Rc::clone(&self.shared),
        }
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

fn load_stored_agent_id() -> Option<String> {
    let window = web_sys::window()?;
    let storage = window.local_storage().ok()??;
    storage.get_item(STORED_AGENT_ID_KEY).ok()?
}

fn store_agent_id(agent_id: &str) -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
    let storage = window
        .local_storage()?
        .ok_or_else(|| JsValue::from_str("No localStorage available"))?;
    storage.set_item(STORED_AGENT_ID_KEY, agent_id)
}

fn store_last_offline_at(timestamp: &str) -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
    let storage = window
        .local_storage()?
        .ok_or_else(|| JsValue::from_str("No localStorage available"))?;
    storage.set_item(STORED_LAST_OFFLINE_AT_KEY, timestamp)
}

#[wasm_bindgen(js_name = set_textarea_value)]
pub fn set_textarea_value(element_id: &str, message: &str) -> Result<(), JsValue> {
    if let Some(window) = web_sys::window()
        && let Some(document) = window.document()
        && let Some(output) = document.get_element_by_id(element_id)
    {
        js_sys::Reflect::set(
            output.as_ref(),
            &JsValue::from_str("value"),
            &JsValue::from_str(message),
        )?;
    }

    Ok(())
}

#[wasm_bindgen(js_name = append_to_textarea)]
pub fn append_to_textarea(element_id: &str, message: &str) -> Result<(), JsValue> {
    if let Some(window) = web_sys::window()
        && let Some(document) = window.document()
        && let Some(output) = document.get_element_by_id(element_id)
    {
        let current_value = js_sys::Reflect::get(output.as_ref(), &JsValue::from_str("value"))?
            .as_string()
            .unwrap_or_default();
        let next_value = if current_value.is_empty() || current_value.starts_with("Workflow module") {
            message.to_string()
        } else {
            format!("{current_value}\n{message}")
        };

        js_sys::Reflect::set(
            output.as_ref(),
            &JsValue::from_str("value"),
            &JsValue::from_str(&next_value),
        )?;

        // Auto-scroll to bottom
        js_sys::Reflect::set(
            output.as_ref(),
            &JsValue::from_str("scrollTop"),
            &js_sys::Reflect::get(output.as_ref(), &JsValue::from_str("scrollHeight"))?,
        )?;
    }

    Ok(())
}
