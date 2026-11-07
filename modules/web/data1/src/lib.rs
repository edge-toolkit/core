#![expect(
    clippy::future_not_send,
    clippy::single_call_fn,
    reason = "browser WASM module: JsFuture is !Send; module-local helpers like wait_for_* are single-use by design"
)]

use edge_toolkit::ws::ServerMessage;
use et_web::{JsResultExt as _, sleep_ms, websocket_url};
use et_ws_wasm_agent::{WsClient, WsClientConfig, append_to_textarea, wait_for_connected};
use futures_util::StreamExt as _;
use tracing::info;
use wasm_bindgen::prelude::*;

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

    #[expect(
        clippy::as_conversions,
        reason = "wasm-bindgen's Closure::wrap takes a `Box<dyn FnMut(...)>`; the cast is required to unsize the Box"
    )]
    let on_message = Closure::wrap(Box::new(move |value: JsValue| {
        let Some(data) = value.as_string() else {
            return;
        };
        et_web::ignore(serde_json::from_str::<ServerMessage>(&data));
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

    // The typed REST client runs against the page origin -- every browser
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
