//! math1-sender: the manual trigger for the math1 family.
//!
//! Plays the fake-agent side of the math1 storage exchange from a browser: uploads the canonical
//! input JSON (embedded at build time from ws-test-server's `data/math1-input.json`, the same bytes
//! every test harness injects) into this agent's own storage bucket, then broadcasts the
//! `math1-input` pointer once a second for a minute so math1 twins started in other tabs (or on
//! other machines connected to the same hub) pick it up, compute, and store their models. This
//! module only sends; each twin's stored `math1-output.json` is the operator's to inspect.

#![expect(
    clippy::future_not_send,
    clippy::single_call_fn,
    reason = "browser WASM module: JsFuture is !Send; module-local helpers are single-use by design"
)]

use et_web::JsResultExt as _;
use et_ws_wasm_agent::{WsClient, WsClientConfig, append_to_textarea};
use js_sys::{Promise, Reflect};
use tracing::info;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

/// The canonical input bytes, embedded from the committed file so there is one source of truth.
const MATH1_INPUT_JSON: &str = include_str!(env!("ET_MATH1_INPUT_PATH"));
/// Storage object name the twins' broadcast pointer names, matching the test harnesses.
const INPUT_FILENAME: &str = "math1-input.json";
/// How many one-second-spaced pointer broadcasts to send before completing.
const BROADCASTS: u32 = 60;

#[wasm_bindgen(start)]
pub fn init() {
    tracing_wasm::set_as_global_default();
    info!("math1-sender module initialized");
}

#[wasm_bindgen]
pub async fn run() -> Result<(), JsValue> {
    let msg = "math1-sender: entered run()";
    log(msg);
    set_module_status(msg)?;

    let ws_url = websocket_url()?;
    let mut config = WsClientConfig::new(ws_url);
    // The wasm-agent retains its server-issued id in localStorage, which is shared across every tab
    // of this origin -- reusing it here would steal the identity of a math1 twin running in another
    // tab, and the hub would then never relay this module's broadcasts to it. A fresh id keeps the
    // sender a distinct peer.
    config.set_use_retained_agent_id(false);
    let mut client = WsClient::new(config);

    client.connect()?;
    wait_for_connected(&client).await?;
    let agent_id = wait_for_agent_id(&client).await?;
    let msg = format!("math1-sender: connected as {agent_id}");
    log(&msg);
    set_module_status(&msg)?;

    // The typed REST client runs against the page origin -- every browser module is served from the
    // same ws-server that owns its storage, so an empty base URL (relative paths) is what we want.
    let rest = et_rest_client::Client::new("");
    let _put_response = rest
        .put_file(&agent_id, INPUT_FILENAME, MATH1_INPUT_JSON.to_string())
        .await
        .js_context("input PUT failed")?;
    let msg = format!("math1-sender: injected the canonical input to /storage/{agent_id}/{INPUT_FILENAME}");
    log(&msg);
    set_module_status(&msg)?;

    let pointer = format!(r#"{{"type":"math1-input","bucket":"{agent_id}","filename":"{INPUT_FILENAME}"}}"#);
    let msg = format!("math1-sender: broadcasting the math1-input pointer every second, {BROADCASTS} times");
    log(&msg);
    set_module_status(&msg)?;
    for round in 1_u32..=BROADCASTS {
        client.send(&pointer)?;
        if round == 1 || round.is_multiple_of(10) {
            let msg = format!("math1-sender: broadcast {round}/{BROADCASTS}");
            log(&msg);
            set_module_status(&msg)?;
        }
        sleep_ms(1000).await?;
    }

    client.disconnect();
    let msg = "math1-sender: workflow complete";
    log(msg);
    set_module_status(msg)?;
    Ok(())
}

fn log(message: &str) {
    let line = format!("[math1-sender] {message}");
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
            et_web::ignore(resolve.call0(&JsValue::NULL));
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
