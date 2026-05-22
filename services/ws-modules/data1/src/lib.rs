#![expect(
    clippy::future_not_send,
    clippy::single_call_fn,
    reason = "browser WASM module: JsFuture is !Send; module-local helpers like wait_for_* are single-use by design"
)]

use std::cell::RefCell;
use std::rc::Rc;

use edge_toolkit::ws::WsMessage;
use et_web::{JsCastExt as _, JsResultExt as _};
use et_ws_wasm_agent::{WsClient, WsClientConfig, append_to_textarea};
use js_sys::{Promise, Reflect};
use tracing::info;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Request, RequestInit, RequestMode, Response};

#[wasm_bindgen(start)]
pub fn init() {
    tracing_wasm::set_as_global_default();
    info!("data1 workflow module initialized");
}

#[wasm_bindgen]
pub async fn run() -> Result<(), JsValue> {
    let msg = "data1: entered run()";
    log(msg);
    set_module_status(msg)?;

    let ws_url = websocket_url()?;
    let mut client = WsClient::new(WsClientConfig::new(ws_url));

    let last_response: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let on_message_boxed: Box<dyn FnMut(JsValue)> = Box::new({
        let last_response = Rc::clone(&last_response);
        move |value: JsValue| {
            let Some(data) = value.as_string() else {
                return;
            };
            let Ok(message) = serde_json::from_str::<WsMessage>(&data) else {
                return;
            };
            if let WsMessage::Response { message } = message {
                *last_response.borrow_mut() = Some(message);
            }
        }
    });
    let on_message = Closure::wrap(on_message_boxed);
    client.set_on_message(on_message.as_ref().clone());

    client.connect()?;
    wait_for_connected(&client).await?;
    let agent_id = wait_for_agent_id(&client).await?;
    let msg = format!("data1: connected as {agent_id}");
    log(&msg);
    set_module_status(&msg)?;

    let filename = "test_data.txt";
    let test_content = format!("Hello from data1 at {}!", js_sys::Date::new_0().to_iso_string());

    // 1. Request Store URL
    log("data1: requesting store URL");
    let store_payload = serde_json::to_string(&WsMessage::StoreFile {
        filename: filename.to_string(),
    })
    .js_context("serialize StoreFile")?;
    client.send(&store_payload)?;
    let store_url = wait_for_response(&last_response, "PUT to ")
        .await?
        .replace("PUT to ", "");

    // 2. Perform PUT
    let msg = format!("data1: storing data to {store_url}");
    log(&msg);
    set_module_status(&msg)?;
    put_file(&store_url, &test_content).await?;

    // 3. Request Fetch URL
    log("data1: requesting fetch URL");
    let fetch_payload = serde_json::to_string(&WsMessage::FetchFile {
        agent_id: agent_id.clone(),
        filename: filename.to_string(),
    })
    .js_context("serialize FetchFile")?;
    client.send(&fetch_payload)?;
    let fetch_url = wait_for_response(&last_response, "GET from ")
        .await?
        .replace("GET from ", "");

    // 4. Perform GET and Verify
    let msg = format!("data1: fetching data from {fetch_url}");
    log(&msg);
    set_module_status(&msg)?;
    let retrieved_content = get_file(&fetch_url).await?;

    if retrieved_content == test_content {
        let msg = "data1: VERIFICATION SUCCESS - data matches!";
        log(msg);
        set_module_status(msg)?;
    } else {
        let msg =
            format!("data1: VERIFICATION FAILURE - data mismatch!\nSent: {test_content}\nGot: {retrieved_content}");
        log(&msg);
        set_module_status(&msg)?;
        return Err(JsValue::from_str("Data mismatch"));
    }

    sleep_ms(2000).await?;
    client.disconnect();
    let msg = "data1: workflow complete";
    log(msg);
    set_module_status(msg)?;
    Ok(())
}

async fn put_file(url: &str, content: &str) -> Result<(), JsValue> {
    let opts = RequestInit::new();
    opts.set_method("PUT");
    opts.set_mode(RequestMode::Cors);
    opts.set_body(&JsValue::from_str(content));

    let request = Request::new_with_str_and_init(url, &opts)?;
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
    let resp_value = JsFuture::from(window.fetch_with_request(&request)).await?;
    let resp: Response = resp_value.dyn_into_msg("PUT response was not a Response")?;

    if resp.status() == 200 {
        Ok(())
    } else {
        Err(JsValue::from_str(&format!("PUT failed with status {}", resp.status())))
    }
}

async fn get_file(url: &str) -> Result<String, JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
    let resp_value = JsFuture::from(window.fetch_with_str(url)).await?;
    let resp: Response = resp_value.dyn_into_msg("GET response was not a Response")?;

    if resp.status() != 200 {
        return Err(JsValue::from_str(&format!("GET failed with status {}", resp.status())));
    }

    let text_promise = resp.text()?;
    let text = JsFuture::from(text_promise).await?;
    Ok(text.as_string().unwrap_or_default())
}

async fn wait_for_response(cell: &Rc<RefCell<Option<String>>>, prefix: &str) -> Result<String, JsValue> {
    for _ in 0_u32..50 {
        let val = cell.borrow().clone();
        if let Some(message) = val
            && message.starts_with(prefix)
        {
            *cell.borrow_mut() = None;
            return Ok(message);
        }
        sleep_ms(100).await?;
    }
    Err(JsValue::from_str("Timeout waiting for server response"))
}

fn log(message: &str) {
    let line = format!("[data1] {message}");
    web_sys::console::log_1(&JsValue::from_str(&line));
}

fn set_module_status(message: &str) -> Result<(), JsValue> {
    append_to_textarea("module-output", message)
}

async fn wait_for_connected(client: &WsClient) -> Result<(), JsValue> {
    for _ in 0_u32..100 {
        if client.get_state() == "connected" {
            return Ok(());
        }
        sleep_ms(100).await?;
    }
    Err(JsValue::from_str("Timed out waiting for websocket connection"))
}

async fn wait_for_agent_id(client: &WsClient) -> Result<String, JsValue> {
    for _ in 0_u32..100 {
        let agent_id = client.get_agent_id();
        if !agent_id.is_empty() {
            return Ok(agent_id);
        }
        sleep_ms(100).await?;
    }
    Err(JsValue::from_str("Timed out waiting for assigned agent_id"))
}

async fn sleep_ms(duration_ms: i32) -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
    let promise = Promise::new(&mut |resolve, _reject| {
        let callback = Closure::once_into_js(move || {
            drop(resolve.call0(&JsValue::NULL));
        });
        let _id: Result<i32, JsValue> =
            window.set_timeout_with_callback_and_timeout_and_arguments_0(callback.unchecked_ref(), duration_ms);
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
