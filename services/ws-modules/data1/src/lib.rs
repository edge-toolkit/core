#![expect(
    clippy::future_not_send,
    clippy::single_call_fn,
    reason = "browser WASM module: JsFuture is !Send; module-local helpers like wait_for_* are single-use by design"
)]

use edge_toolkit::ws::ServerMessage;
use et_web::JsResultExt as _;
use et_ws_wasm_agent::{WsClient, WsClientConfig, append_to_textarea};
use futures_util::StreamExt as _;
use js_sys::{Promise, Reflect};
use tracing::info;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

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

    /*
    let last_response: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let on_message_boxed: Box<dyn FnMut(JsValue)> = Box::new({
        let last_response = Rc::clone(&last_response);
        move |value: JsValue| {
            let Some(data) = value.as_string() else {
                return;
            };
            let Ok(message) = serde_json::from_str::<ServerMessage>(&data) else {
                return;
            };
            if let ServerMessage::Response { message } = message {
                *last_response.borrow_mut() = Some(message);
            }
        }
    });
    let on_message = Closure::wrap(on_message_boxed);

    */
    #[expect(
        clippy::as_conversions,
        reason = "wasm-bindgen's Closure::wrap takes a `Box<dyn FnMut(...)>`; the cast is required to unsize the Box"
    )]
    let on_message = Closure::wrap(Box::new(move |value: JsValue| {
        let Some(data) = value.as_string() else {
            return;
        };
        drop(serde_json::from_str::<ServerMessage>(&data));
    }) as Box<dyn FnMut(JsValue)>);

    client.set_on_message(on_message.as_ref().clone());

    client.connect()?;
    wait_for_connected(&client).await?;
    let agent_id = wait_for_agent_id(&client).await?;
    let msg = format!("data1: connected as {agent_id}");
    log(&msg);
    set_module_status(&msg)?;

    let filename = "test_data.txt";
    let test_content = format!("Hello from data1 at {}!", js_sys::Date::new_0().to_iso_string());

    // The typed REST client runs against the page origin — every browser
    // module is served from the same ws-server that owns its storage, so
    // an empty base URL (relative paths) is what we want.
    let rest = et_rest_client::Client::new("");

    let msg = format!("data1: storing data to /storage/{agent_id}/{filename}");
    log(&msg);
    set_module_status(&msg)?;
    let _put_response = rest
        .put_file(&agent_id, filename, test_content.clone())
        .await
        .js_context("PUT failed")?;

    let msg = format!("data1: fetching data from /storage/{agent_id}/{filename}");
    log(&msg);
    set_module_status(&msg)?;
    let response = rest.get_file(&agent_id, filename).await.js_context("GET failed")?;
    let retrieved_bytes = collect_stream(response.into_inner()).await?;
    let retrieved_content = String::from_utf8(retrieved_bytes).js_context("non-UTF-8 body")?;

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

async fn collect_stream(mut stream: et_rest_client::ByteStream) -> Result<Vec<u8>, JsValue> {
    let mut buf = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.js_context("stream chunk")?;
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
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
