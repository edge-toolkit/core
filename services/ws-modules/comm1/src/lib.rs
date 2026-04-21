use std::cell::RefCell;
use std::rc::Rc;

use edge_toolkit::ws::{AgentConnectionState, AgentSummary, WsMessage};
use et_ws_wasm_agent::{WsClient, WsClientConfig, append_to_textarea};
use js_sys::{Promise, Reflect};
use serde_json::json;
use tracing::info;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

const LIST_AGENTS_POLL_MS: i32 = 1_000;
const MESSAGE_PAUSE_MS: i32 = 3_000;

#[wasm_bindgen(start)]
pub fn init() {
    tracing_wasm::set_as_global_default();
    info!("comm1 workflow module initialized");
}

#[wasm_bindgen]
pub async fn run() -> Result<(), JsValue> {
    log("comm1: entered run()")?;

    let ws_url = websocket_url()?;
    log(&format!("comm1: resolved websocket URL: {ws_url}"))?;

    let mut client = WsClient::new(WsClientConfig::new(ws_url));
    let self_agent_id = Rc::new(RefCell::new(String::new()));
    let other_connected_agents: Rc<RefCell<Vec<AgentSummary>>> = Rc::new(RefCell::new(Vec::new()));

    let on_message = Closure::wrap(Box::new({
        let self_agent_id = self_agent_id.clone();
        let other_connected_agents = other_connected_agents.clone();
        move |value: JsValue| {
            let Some(data) = value.as_string() else {
                return;
            };

            let Ok(message) = serde_json::from_str::<WsMessage>(&data) else {
                return;
            };

            match message {
                WsMessage::ListAgentsResponse { agents } => {
                    let own_id = self_agent_id.borrow().clone();
                    let others = agents
                        .into_iter()
                        .filter(|agent| {
                            agent.state == AgentConnectionState::Connected
                                && !own_id.is_empty()
                                && agent.agent_id != own_id
                        })
                        .collect::<Vec<_>>();
                    *other_connected_agents.borrow_mut() = others;
                }
                WsMessage::AgentMessage {
                    message_id,
                    from_agent_id,
                    scope,
                    server_received_at,
                    message,
                } => {
                    let summary =
                        serde_json::to_string(&message).unwrap_or_else(|_| String::from("<unprintable message>"));
                    let line = format!(
                        "comm1: received {:?} message {} from {} at {}: {}",
                        scope, message_id, from_agent_id, server_received_at, summary
                    );
                    web_sys::console::log_1(&JsValue::from_str(&line));
                    let _ = set_module_status(&line);
                }
                WsMessage::MessageStatus {
                    message_id,
                    status,
                    detail,
                } => {
                    let line = format!("comm1: message status update {:?} {:?}: {}", message_id, status, detail);
                    web_sys::console::log_1(&JsValue::from_str(&line));
                    let _ = set_module_status(&line);
                }
                WsMessage::Invalid { message_id, detail } => {
                    let line = format!("comm1: invalid server response {:?}: {}", message_id, detail);
                    web_sys::console::warn_1(&JsValue::from_str(&line));
                    let _ = set_module_status(&line);
                }
                _ => {}
            }
        }
    }) as Box<dyn FnMut(JsValue)>);
    client.set_on_message(on_message.as_ref().clone());

    client.connect()?;
    wait_for_connected(&client).await?;
    let agent_id = wait_for_agent_id(&client).await?;
    *self_agent_id.borrow_mut() = agent_id.clone();
    let msg = format!("comm1: websocket connected with agent_id={agent_id}");
    log(&msg)?;
    set_module_status(&msg)?;

    let target_agent = loop {
        client.request_list_agents()?;
        sleep_ms(LIST_AGENTS_POLL_MS).await?;

        let agents = other_connected_agents.borrow().clone();
        if let Some(agent) = agents.first() {
            break agent.clone();
        }
    };

    let msg = format!(
        "comm1: found connected peer agent {}; sending broadcast",
        target_agent.agent_id
    );
    log(&msg)?;
    set_module_status(&msg)?;
    client.broadcast_message(json!({
        "module": "comm1",
        "step": "broadcast",
        "from_agent_id": agent_id,
        "message": "comm1 broadcast to all other connected agents"
    }))?;

    sleep_ms(MESSAGE_PAUSE_MS).await?;

    let msg = format!("comm1: sending direct message to {}", target_agent.agent_id);
    log(&msg)?;
    set_module_status(&msg)?;
    client.send_agent_message(
        target_agent.agent_id.clone(),
        json!({
            "module": "comm1",
            "step": "direct",
            "from_agent_id": agent_id,
            "message": "comm1 direct message"
        }),
    )?;

    sleep_ms(MESSAGE_PAUSE_MS).await?;
    client.disconnect();
    let msg = "comm1: workflow complete";
    log(msg)?;
    set_module_status(msg)?;
    Ok(())
}

fn log(message: &str) -> Result<(), JsValue> {
    let line = format!("[comm1] {message}");
    web_sys::console::log_1(&JsValue::from_str(&line));
    Ok(())
}

fn set_module_status(message: &str) -> Result<(), JsValue> {
    append_to_textarea("module-output", message)
}

async fn wait_for_connected(client: &WsClient) -> Result<(), JsValue> {
    for _ in 0..100 {
        if client.get_state() == "connected" {
            return Ok(());
        }
        sleep_ms(100).await?;
    }

    Err(JsValue::from_str("Timed out waiting for websocket connection"))
}

async fn wait_for_agent_id(client: &WsClient) -> Result<String, JsValue> {
    for _ in 0..100 {
        let agent_id = client.get_client_id();
        if !agent_id.is_empty() {
            return Ok(agent_id);
        }
        sleep_ms(100).await?;
    }

    Err(JsValue::from_str("Timed out waiting for assigned agent_id"))
}

async fn sleep_ms(duration_ms: i32) -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
    let promise = Promise::new(&mut |resolve, reject| {
        let callback = Closure::once_into_js(move || {
            let _ = resolve.call0(&JsValue::NULL);
        });

        if let Err(error) =
            window.set_timeout_with_callback_and_timeout_and_arguments_0(callback.unchecked_ref(), duration_ms)
        {
            let _ = reject.call1(&JsValue::NULL, &error);
        }
    });
    JsFuture::from(promise).await.map(|_| ())
}

fn websocket_url() -> Result<String, JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
    let location = Reflect::get(window.as_ref(), &JsValue::from_str("location"))?;
    let protocol = Reflect::get(&location, &JsValue::from_str("protocol"))?
        .as_string()
        .ok_or_else(|| JsValue::from_str("window.location.protocol is unavailable"))?;
    let host = Reflect::get(&location, &JsValue::from_str("host"))?
        .as_string()
        .ok_or_else(|| JsValue::from_str("window.location.host is unavailable"))?;
    let ws_protocol = if protocol == "https:" { "wss:" } else { "ws:" };
    Ok(format!("{ws_protocol}//{host}/ws"))
}
