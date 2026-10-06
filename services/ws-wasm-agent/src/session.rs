//! The client's session lifecycle that JavaScript never calls directly: heartbeats, reconnects, the offline queue.
//!
//! These live in `WsClient`'s second, non-exported `impl` block, beside the typed message helpers `wasm_bindgen`
//! cannot export, so the exported block in the crate root holds only the API a page sees.

use std::rc::Rc;

use edge_toolkit::ws::ClientMessage;
use et_web::JsResultExt as _;
use tracing::{error, info, warn};
use wasm_bindgen::prelude::*;

use crate::dom::store_last_offline_at;
use crate::{ConnectionState, MAX_OFFLINE_QUEUE_LEN, WsClient, WsClientConfig};

#[expect(
    clippy::multiple_inherent_impl,
    reason = "second block holds methods that wasm_bindgen can't export (generics, serde_json::Value, internals)"
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

    pub fn send_agent_message<T>(&self, to_agent_id: T, message: serde_json::Value) -> Result<(), JsValue>
    where
        T: Into<String>,
    {
        let payload = serde_json::to_string(&ClientMessage::SendAgentMessage {
            to_agent_id: to_agent_id.into(),
            message,
        })
        .js_context("Failed to serialize direct message")?;
        self.send(&payload)
    }

    pub fn send_client_event<C, A>(&self, capability: C, action: A, details: serde_json::Value) -> Result<(), JsValue>
    where
        C: Into<String>,
        A: Into<String>,
    {
        let message = ClientMessage::ClientEvent {
            capability: capability.into(),
            action: action.into(),
            details,
        };
        let payload = serde_json::to_string(&message).js_context("Failed to serialize client event")?;
        self.send(&payload)
    }

    pub(crate) fn start_alive_interval(&self) {
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

    #[expect(
        clippy::cognitive_complexity,
        reason = "the score is info!/warn! expansion; the body branches once"
    )]
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

    pub(crate) fn stop_alive_interval(&self) {
        let mut state = self.shared.borrow_mut();
        if let Some(interval_id) = state.alive_interval_id.take() {
            if let Some(window) = web_sys::window() {
                window.clear_interval_with_handle(interval_id);
            }
            info!("Stopped alive interval");
        }
    }

    #[expect(
        clippy::cognitive_complexity,
        reason = "three log calls' expansion on top of the backoff decision, whose state must update under one borrow"
    )]
    pub(crate) fn handle_disconnect(&self) {
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

    pub(crate) fn notify_state_change(&self) {
        let state_label = self.get_state();
        let state = self.shared.borrow();
        if let Some(callback) = &state.on_state_change_callback
            && let Some(function) = callback.dyn_ref::<js_sys::Function>()
        {
            let _called: Result<JsValue, JsValue> = function.call1(&JsValue::NULL, &JsValue::from_str(&state_label));
        }
    }

    pub(crate) fn send_connect_message(&self) -> Result<(), JsValue> {
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

    #[expect(
        clippy::cognitive_complexity,
        reason = "the score is warn!/info! expansion; the body branches once"
    )]
    pub(crate) fn enqueue_offline_message(&self, message: &str) {
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

    #[expect(
        clippy::cognitive_complexity,
        reason = "the score is mostly warn!/info! expansion around a drain loop with two early returns"
    )]
    pub(crate) fn flush_offline_queue(&self) {
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
        clippy::cognitive_complexity,
        clippy::unused_self,
        reason = "&self mirrors the other lifecycle methods, state lives in localStorage; score is log-macro expansion"
    )]
    pub(crate) fn record_offline(&self) {
        let timestamp = chrono::Utc::now().to_rfc3339();
        match store_last_offline_at(&timestamp) {
            Ok(()) => info!("Recorded websocket offline transition at {}", timestamp),
            Err(error) => warn!("Failed to record websocket offline transition: {:?}", error),
        }
    }

    #[expect(
        clippy::cognitive_complexity,
        reason = "the score is warn!/error! expansion; the body branches twice"
    )]
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

    pub(crate) fn cancel_reconnect(&self) {
        let mut state = self.shared.borrow_mut();
        if let Some(timeout_id) = state.reconnect_timeout_id.take()
            && let Some(window) = web_sys::window()
        {
            window.clear_timeout_with_handle(timeout_id);
        }
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
                use_retained_agent_id: self.config.use_retained_agent_id,
            },
            agent_id: Rc::clone(&self.agent_id),
            shared: Rc::clone(&self.shared),
        }
    }
}
