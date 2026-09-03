//! math1: federated-averaging (`FedAvg`) demo in a browser WASM module.
//!
//! Storage-driven twin family: a fake agent injects the canonical input JSON (client datasets +
//! hyperparameters) into ws-server storage and broadcasts a `math1-input` pointer over the hub.
//! This module waits for that pointer, reads the input from storage, runs the `FedAvg` kernel --
//! rounds of local full-batch gradient-descent epochs per client merged with a sample-count-weighted
//! average, only `+ - * /` on f64 so the result is bit-identical on every IEEE-754 runtime -- and
//! stores the global model to `math1-output.json` in its own bucket, where the test harness reads
//! and verifies it. The math1 twins in the other guest languages run the same protocol and kernel.

#![expect(
    clippy::float_arithmetic,
    clippy::future_not_send,
    clippy::single_call_fn,
    reason = "browser WASM module: JsFuture is !Send; the FedAvg kernel is float math; helpers are single-use"
)]

use std::cell::RefCell;
use std::rc::Rc;

use et_web::{JsResultExt as _, sleep_ms, websocket_url};
use et_ws_wasm_agent::{WsClient, WsClientConfig, append_to_textarea, wait_for_connected};
use futures_util::StreamExt as _;
use serde::Deserialize;
use tracing::info;
use wasm_bindgen::prelude::*;

/// The canonical input: per-client (feature, target) samples plus the training hyperparameters.
#[derive(Deserialize)]
struct Math1Input {
    clients: Vec<Vec<(f64, f64)>>,
    rounds: u32,
    epochs: u32,
    learning_rate: f64,
}

/// The broadcast pointer naming the storage bucket + filename the input JSON was injected at.
#[derive(Clone, Deserialize)]
struct InputPointer {
    bucket: String,
    filename: String,
}

#[wasm_bindgen(start)]
pub fn init() {
    tracing_wasm::set_as_global_default();
    info!("math1 FedAvg module initialized");
}

/// Sample count as f64, accumulated additively to avoid an integer-to-float cast.
fn sample_count(samples: &[(f64, f64)]) -> f64 {
    samples.iter().fold(0.0_f64, |count, _| count + 1.0)
}

/// Runs the `FedAvg` simulation on `input` and returns the final global (weight, bias).
fn fed_avg(input: &Math1Input) -> (f64, f64) {
    let mut weight = 0.0_f64;
    let mut bias = 0.0_f64;
    let total_samples: f64 = input
        .clients
        .iter()
        .fold(0.0_f64, |acc, samples| acc + sample_count(samples));
    for _ in 0_u32..input.rounds {
        let mut merged_weight = 0.0_f64;
        let mut merged_bias = 0.0_f64;
        for samples in &input.clients {
            let count = sample_count(samples);
            let mut client_weight = weight;
            let mut client_bias = bias;
            for _ in 0_u32..input.epochs {
                let mut grad_weight = 0.0_f64;
                let mut grad_bias = 0.0_f64;
                for &(feature, target) in samples {
                    let residual = client_weight * feature + client_bias - target;
                    grad_weight += residual * feature;
                    grad_bias += residual;
                }
                client_weight -= input.learning_rate * (2.0 * grad_weight / count);
                client_bias -= input.learning_rate * (2.0 * grad_bias / count);
            }
            merged_weight += client_weight * count;
            merged_bias += client_bias * count;
        }
        weight = merged_weight / total_samples;
        bias = merged_bias / total_samples;
    }
    (weight, bias)
}

#[wasm_bindgen]
pub async fn run() -> Result<(), JsValue> {
    let msg = "math1: entered run()";
    log(msg);
    set_module_status(msg)?;

    let ws_url = websocket_url()?;
    let mut client = WsClient::new(WsClientConfig::new(ws_url));

    // Capture the math1-input pointer broadcast; every other frame is ignored.
    let pointer_slot: Rc<RefCell<Option<InputPointer>>> = Rc::new(RefCell::new(None));
    #[expect(
        clippy::as_conversions,
        reason = "wasm-bindgen's Closure::wrap takes a `Box<dyn FnMut(...)>`; the cast is required to unsize the Box"
    )]
    let on_message = Closure::wrap(Box::new({
        let pointer_slot = Rc::clone(&pointer_slot);
        move |value: JsValue| {
            let Some(data) = value.as_string() else {
                return;
            };
            let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) else {
                return;
            };
            if json.get("type").and_then(serde_json::Value::as_str) == Some("math1-input")
                && let Ok(pointer) = serde_json::from_value::<InputPointer>(json)
            {
                *pointer_slot.borrow_mut() = Some(pointer);
            }
        }
    }) as Box<dyn FnMut(JsValue)>);
    client.set_on_message(on_message.as_ref().clone());

    client.connect()?;
    wait_for_connected(&client).await?;
    let agent_id = wait_for_agent_id(&client).await?;
    let msg = format!("math1: connected as {agent_id}");
    log(&msg);
    set_module_status(&msg)?;

    let msg = "math1: waiting for the math1-input pointer broadcast";
    log(msg);
    set_module_status(msg)?;
    let pointer = wait_for_pointer(&pointer_slot).await?;

    let msg = format!(
        "math1: reading input from /storage/{}/{}",
        pointer.bucket, pointer.filename
    );
    log(&msg);
    set_module_status(&msg)?;
    // The typed REST client runs against the page origin -- every browser module is served from the
    // same ws-server that owns its storage, so an empty base URL (relative paths) is what we want.
    let rest = et_rest_client::Client::new("");
    let response = rest
        .get_file(&pointer.bucket, &pointer.filename)
        .await
        .js_context("input GET failed")?;
    let input_bytes = collect_stream(response.into_inner()).await?;
    let input: Math1Input = serde_json::from_slice(&input_bytes).js_context("input JSON parse failed")?;

    let msg = format!(
        "math1: running FedAvg - {} clients x {} rounds x {} local epochs",
        input.clients.len(),
        input.rounds,
        input.epochs
    );
    log(&msg);
    set_module_status(&msg)?;
    let (weight, bias) = fed_avg(&input);
    let msg = format!("math1: global model weight={weight} bias={bias}");
    log(&msg);
    set_module_status(&msg)?;

    let output = serde_json::json!({ "module": "math1", "weight": weight, "bias": bias }).to_string();
    let _put_response = rest
        .put_file(&agent_id, "math1-output.json", output)
        .await
        .js_context("output PUT failed")?;
    let msg = format!("math1: stored the global model to /storage/{agent_id}/math1-output.json");
    log(&msg);
    set_module_status(&msg)?;

    sleep_ms(2000).await?;
    client.disconnect();
    let msg = "math1: workflow complete";
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
    let line = format!("[math1] {message}");
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

async fn wait_for_pointer(slot: &Rc<RefCell<Option<InputPointer>>>) -> Result<InputPointer, JsValue> {
    for _ in 0_u32..100 {
        if let Some(pointer) = slot.borrow().clone() {
            return Ok(pointer);
        }
        sleep_ms(100).await?;
    }
    Err(JsValue::from_str("Timed out waiting for the math1-input pointer"))
}
