use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use edge_toolkit::ws::{ConnectStatus, WsMessage};
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
#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
}

pub fn js_number_field(value: &JsValue, field: &str) -> Option<f64> {
    js_sys::Reflect::get(value, &JsValue::from_str(field))
        .ok()
        .and_then(|field_value| field_value.as_f64())
}

pub fn js_bool_field(value: &JsValue, field: &str) -> Option<bool> {
    js_sys::Reflect::get(value, &JsValue::from_str(field))
        .ok()
        .and_then(|field_value| field_value.as_bool())
}

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
impl WsClientConfig {
    #[wasm_bindgen(constructor)]
    pub fn new(server_url: String) -> WsClientConfig {
        WsClientConfig {
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
    pub fn new(config: WsClientConfig) -> WsClient {
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

        WsClient {
            config,
            agent_id: Rc::new(RefCell::new(agent_id)),
            shared,
        }
    }

    /// Connect to the WebSocket server
    #[wasm_bindgen]
    pub fn connect(&mut self) -> Result<(), JsValue> {
        info!("Connecting to WebSocket server: {}", self.config.server_url);

        let _window = web_sys::window().ok_or("No window available")?;
        let socket = WebSocket::new(&self.config.server_url)
            .map_err(|e| JsValue::from_str(&format!("Failed to create WebSocket: {:?}", e)))?;

        // Set binary type to arraybuffer
        socket.set_binary_type(web_sys::BinaryType::Arraybuffer);

        // Store the socket
        {
            let mut s = self.shared.borrow_mut();
            s.socket = Some(socket.clone());
            s.state = ConnectionState::Connecting;
            s.manual_disconnect = false;
        }
        self.notify_state_change();

        // Set up event handlers
        let on_open = Closure::wrap(Box::new({
            let shared = self.shared.clone();
            let initial_delay = self.config.initial_reconnect_delay_ms;
            let cli_ptr = self.clone();
            move |_event: Event| {
                info!("WebSocket connected");
                {
                    let mut s = shared.borrow_mut();
                    s.state = ConnectionState::Connected;
                    s.reconnect_attempts = 0;
                    s.reconnect_delay_ms = initial_delay;
                    if let Some(timeout_id) = s.reconnect_timeout_id.take()
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
        }) as Box<dyn FnMut(Event)>);

        let on_message = Closure::wrap(Box::new({
            let shared = self.shared.clone();
            let retained_agent_id = self.agent_id.clone();
            move |event: MessageEvent| {
                info!("WebSocket message received");
                if let Some(data) = event.data().as_string() {
                    info!("Received: {}", data);
                    // Try to parse and handle the message
                    if let Ok(msg) = serde_json::from_str::<WsMessage>(&data) {
                        match msg {
                            WsMessage::ConnectAck { agent_id, status } => {
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
                            WsMessage::Response { message } => {
                                info!("Server response: {}", message);
                            }
                            WsMessage::ListAgents => {
                                warn!("Unexpected list_agents message from server");
                            }
                            WsMessage::ListAgentsResponse { agents } => {
                                info!("Server returned {} agents", agents.len());
                            }
                            WsMessage::SendAgentMessage { .. } => {
                                warn!("Unexpected send_agent_message request from server");
                            }
                            WsMessage::BroadcastMessage { .. } => {
                                warn!("Unexpected broadcast_message request from server");
                            }
                            WsMessage::AgentMessage {
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
                            WsMessage::MessageAck { .. } => {
                                warn!("Unexpected message_ack from server");
                            }
                            WsMessage::MessageStatus {
                                message_id,
                                status,
                                detail,
                            } => {
                                info!("Message status update {:?} {:?}: {}", message_id, status, detail);
                            }
                            WsMessage::Invalid { message_id, detail } => {
                                warn!("Invalid server message {:?}: {}", message_id, detail);
                            }
                            WsMessage::Alive { .. } => {
                                warn!("Unexpected alive message from server");
                            }
                            WsMessage::ClientEvent { .. } => {
                                warn!("Unexpected client_event message from server");
                            }
                            WsMessage::Connect { .. } => {
                                warn!("Unexpected connect message from server");
                            }
                            WsMessage::StoreFile { .. } | WsMessage::FetchFile { .. } => {
                                warn!("Unexpected file storage request from server");
                            }
                        }
                    }
                    // Notify callback if set
                    let s = shared.borrow();
                    if let Some(ref callback) = s.on_message_callback
                        && let Some(function) = callback.dyn_ref::<js_sys::Function>()
                    {
                        let _ = function.call1(&JsValue::NULL, &JsValue::from_str(&data));
                    }
                }
            }
        }) as Box<dyn FnMut(MessageEvent)>);

        let on_error = Closure::wrap(Box::new({
            let mut cli_ptr = self.clone();
            move |_event: Event| {
                error!("WebSocket error occurred");
                cli_ptr.handle_disconnect();
            }
        }) as Box<dyn FnMut(Event)>);

        let on_close = Closure::wrap(Box::new({
            let mut cli_ptr = self.clone();
            move |_event: Event| {
                info!("WebSocket closed");
                cli_ptr.handle_disconnect();
            }
        }) as Box<dyn FnMut(Event)>);

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
            let mut s = self.shared.borrow_mut();
            s.manual_disconnect = true;
            if let Some(ref socket) = s.socket {
                let _ = socket.close();
            }
            s.socket = None;
            s.state = ConnectionState::Disconnected;
        }
        self.notify_state_change();
    }

    /// Send an alive message to the server
    #[wasm_bindgen]
    pub fn send_alive(&self) -> Result<(), JsValue> {
        let s = self.shared.borrow();
        if s.state != ConnectionState::Connected {
            return Err(JsValue::from_str("Not connected"));
        }

        let timestamp = chrono::Utc::now().to_rfc3339();
        let msg = WsMessage::Alive { timestamp };

        let json = serde_json::to_string(&msg)
            .map_err(|e| JsValue::from_str(&format!("Failed to serialize message: {}", e)))?;

        if let Some(ref socket) = s.socket {
            socket
                .send_with_str(&json)
                .map_err(|e| JsValue::from_str(&format!("Failed to send message: {:?}", e)))?;
            info!("Alive message sent: {}", json);
        }

        Ok(())
    }

    /// Send a custom message to the server
    #[wasm_bindgen]
    pub fn send(&self, message: &str) -> Result<(), JsValue> {
        let should_queue = {
            let s = self.shared.borrow();
            s.state != ConnectionState::Connected || s.socket.is_none()
        };

        if should_queue {
            self.enqueue_offline_message(message);
            return Ok(());
        }

        let send_result = {
            let s = self.shared.borrow();
            s.socket
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
                    "Failed to send message immediately; queued for retry: {:?}",
                    error
                )))
            }
        }
    }

    /// Get the current connection state
    #[wasm_bindgen]
    pub fn get_state(&self) -> String {
        match self.shared.borrow().state {
            ConnectionState::Disconnected => "disconnected".to_string(),
            ConnectionState::Connecting => "connecting".to_string(),
            ConnectionState::Connected => "connected".to_string(),
            ConnectionState::Reconnecting => "reconnecting".to_string(),
        }
    }

    /// Get the client ID
    #[wasm_bindgen]
    pub fn get_client_id(&self) -> String {
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

        let interval_ms = self.config.alive_interval_ms as i32;
        let cli_ptr = self.clone();
        let interval_closure = Closure::wrap(Box::new(move || {
            if let Err(error) = cli_ptr.send_alive() {
                warn!("Failed to send alive keepalive: {:?}", error);
            }
        }) as Box<dyn FnMut()>);

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
        let mut s = self.shared.borrow_mut();
        if let Some(interval_id) = s.alive_interval_id.take() {
            if let Some(window) = web_sys::window() {
                window.clear_interval_with_handle(interval_id);
            }
            info!("Stopped alive interval");
        }
    }

    fn handle_disconnect(&mut self) {
        self.stop_alive_interval();
        let manual_disconnect = {
            let mut s = self.shared.borrow_mut();
            s.socket = None;
            s.state = ConnectionState::Disconnected;
            s.manual_disconnect
        };
        self.record_offline();
        self.notify_state_change();

        if manual_disconnect {
            info!("Manual websocket disconnect; skipping reconnect");
            return;
        }

        // Attempt reconnection with exponential backoff
        let mut do_reconnect = false;
        let mut next_delay = 0;
        let mut curr_attempt = 0;
        {
            let mut s = self.shared.borrow_mut();
            if s.reconnect_attempts < self.config.max_reconnect_attempts {
                s.state = ConnectionState::Reconnecting;
                next_delay = s.reconnect_delay_ms;
                s.reconnect_delay_ms = (s.reconnect_delay_ms * 2).min(30000);
                s.reconnect_attempts += 1;
                curr_attempt = s.reconnect_attempts;
                do_reconnect = true;
            }
        }
        if do_reconnect {
            self.notify_state_change();
            info!("Attempting reconnection {} in {}ms", curr_attempt, next_delay);
            self.schedule_reconnect(next_delay as i32);
        } else {
            error!("Max reconnection attempts reached");
        }
    }

    fn notify_state_change(&self) {
        let state = self.get_state();
        let s = self.shared.borrow();
        if let Some(ref callback) = s.on_state_change_callback
            && let Some(function) = callback.dyn_ref::<js_sys::Function>()
        {
            let _ = function.call1(&JsValue::NULL, &JsValue::from_str(&state));
        }
    }

    fn send_connect_message(&self) -> Result<(), JsValue> {
        let s = self.shared.borrow();
        let msg = WsMessage::Connect {
            agent_id: self.agent_id.borrow().clone(),
        };

        let json = serde_json::to_string(&msg)
            .map_err(|e| JsValue::from_str(&format!("Failed to serialize connect message: {}", e)))?;

        if let Some(ref socket) = s.socket {
            socket
                .send_with_str(&json)
                .map_err(|e| JsValue::from_str(&format!("Failed to send connect message: {:?}", e)))?;
            info!("Connect message sent: {}", json);
        }

        Ok(())
    }

    fn enqueue_offline_message(&self, message: &str) {
        let mut s = self.shared.borrow_mut();
        if s.offline_queue.len() == MAX_OFFLINE_QUEUE_LEN {
            s.offline_queue.pop_front();
            warn!(
                "Offline websocket queue reached {} messages; dropping oldest entry",
                MAX_OFFLINE_QUEUE_LEN
            );
        }
        s.offline_queue.push_back(message.to_string());
        info!(
            "Queued websocket message while offline (queue_len={}): {}",
            s.offline_queue.len(),
            message
        );
    }

    fn flush_offline_queue(&self) {
        loop {
            let next_message = {
                let mut s = self.shared.borrow_mut();
                if s.state != ConnectionState::Connected || s.socket.is_none() {
                    return;
                }
                s.offline_queue.pop_front()
            };

            let Some(message) = next_message else {
                return;
            };

            let send_result = {
                let s = self.shared.borrow();
                s.socket
                    .as_ref()
                    .ok_or_else(|| JsValue::from_str("No websocket available"))
                    .and_then(|socket| {
                        socket
                            .send_with_str(&message)
                            .map_err(|error| JsValue::from_str(&format!("Failed to flush queued message: {:?}", error)))
                    })
            };

            if let Err(error) = send_result {
                warn!("Failed to flush queued websocket message; re-queueing: {:?}", error);
                let mut s = self.shared.borrow_mut();
                s.offline_queue.push_front(message);
                return;
            }

            info!("Flushed queued websocket message: {}", message);
        }
    }

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
        let reconnect_closure = Closure::once(Box::new(move || {
            if let Err(error) = cli_ptr.connect() {
                error!("Reconnect attempt failed: {:?}", error);
            }
        }) as Box<dyn FnOnce()>);

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
        let mut s = self.shared.borrow_mut();
        if let Some(timeout_id) = s.reconnect_timeout_id.take()
            && let Some(window) = web_sys::window()
        {
            window.clear_timeout_with_handle(timeout_id);
        }
    }
}

impl WsClient {
    pub fn request_list_agents(&self) -> Result<(), JsValue> {
        let payload = serde_json::to_string(&WsMessage::ListAgents)
            .map_err(|error| JsValue::from_str(&format!("Failed to serialize list_agents: {error}")))?;
        self.send(&payload)
    }

    pub fn broadcast_message(&self, message: serde_json::Value) -> Result<(), JsValue> {
        let payload = serde_json::to_string(&WsMessage::BroadcastMessage { message })
            .map_err(|error| JsValue::from_str(&format!("Failed to serialize broadcast message: {error}")))?;
        self.send(&payload)
    }

    pub fn send_agent_message(
        &self,
        to_agent_id: impl Into<String>,
        message: serde_json::Value,
    ) -> Result<(), JsValue> {
        let payload = serde_json::to_string(&WsMessage::SendAgentMessage {
            to_agent_id: to_agent_id.into(),
            message,
        })
        .map_err(|error| JsValue::from_str(&format!("Failed to serialize direct message: {error}")))?;
        self.send(&payload)
    }

    pub fn send_client_event(
        &self,
        capability: impl Into<String>,
        action: impl Into<String>,
        details: serde_json::Value,
    ) -> Result<(), JsValue> {
        let message = WsMessage::ClientEvent {
            capability: capability.into(),
            action: action.into(),
            details,
        };
        let payload = serde_json::to_string(&message)
            .map_err(|error| JsValue::from_str(&format!("Failed to serialize client event: {error}")))?;
        self.send(&payload)
    }
}

// Implement Clone for WsClient (required for closures)
impl Clone for WsClient {
    fn clone(&self) -> WsClient {
        WsClient {
            config: WsClientConfig {
                server_url: self.config.server_url.clone(),
                alive_interval_ms: self.config.alive_interval_ms,
                max_reconnect_attempts: self.config.max_reconnect_attempts,
                initial_reconnect_delay_ms: self.config.initial_reconnect_delay_ms,
            },
            agent_id: self.agent_id.clone(),
            shared: self.shared.clone(),
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
