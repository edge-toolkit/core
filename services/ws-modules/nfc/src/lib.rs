#![expect(
    clippy::future_not_send,
    clippy::single_call_fn,
    reason = "browser WASM module: JsFuture is !Send; module-local helpers like wait_for_* are single-use by design"
)]

use et_web::{JsFunctionExt as _, JsPromiseExt as _};
use et_ws_wasm_agent::{WsClient, WsClientConfig, set_textarea_value};
use js_sys::{Promise, Reflect};
use serde_json::json;
use tracing::info;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

const NFC_SCAN_TIMEOUT_MS: i32 = 60_000;

#[wasm_bindgen]
pub struct NfcScanResult {
    serial_number: String,
    record_summary: String,
}

#[wasm_bindgen]
impl NfcScanResult {
    #[wasm_bindgen(js_name = scanOnce)]
    pub async fn scan_once() -> Result<Self, JsValue> {
        Self::scan_once_with_timeout(NFC_SCAN_TIMEOUT_MS).await
    }

    #[wasm_bindgen(js_name = scanOnceWithTimeout)]
    pub async fn scan_once_with_timeout(timeout_ms: i32) -> Result<Self, JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
        let ndef_ctor = js_sys::Reflect::get(&window, &JsValue::from_str("NDEFReader"))
            .ok()
            .filter(|value| !value.is_undefined() && !value.is_null())
            .ok_or_else(|| JsValue::from_str("Web NFC is not available in this browser context"))?;

        let constructor = ndef_ctor.into_function("NDEFReader constructor")?;
        let reader = js_sys::Reflect::construct(&constructor, &js_sys::Array::new())?;

        let scan = js_sys::Reflect::get(&reader, &JsValue::from_str("scan"))?.into_function("NDEFReader.scan")?;
        let scan_promise = scan.call0(&reader)?.into_promise("NDEFReader.scan")?;
        let _scan_result: JsValue = JsFuture::from(scan_promise).await?;

        let promise = js_sys::Promise::new(&mut |resolve, reject| {
            let reject_for_timeout = reject.clone();
            #[expect(
                clippy::integer_division,
                clippy::integer_division_remainder_used,
                reason = "convert ms timeout to seconds for human-readable error message; not a crypto context"
            )]
            let timeout_seconds = timeout_ms / 1000_i32;
            let timeout_box: Box<dyn FnOnce()> = Box::new(move || {
                et_web::ignore(reject_for_timeout.call1(
                    &JsValue::NULL,
                    &JsValue::from_str(&format!("NFC scan timed out after {timeout_seconds} seconds")),
                ));
            });
            let timeout_closure = Closure::once(timeout_box);

            if let Some(window) = web_sys::window() {
                et_web::ignore(window.set_timeout_with_callback_and_timeout_and_arguments_0(
                    timeout_closure.as_ref().unchecked_ref(),
                    timeout_ms,
                ));
            }

            let reject_for_error = reject;

            let on_reading_box: Box<dyn FnOnce(JsValue)> = Box::new(move |event: JsValue| {
                let serial_number = js_sys::Reflect::get(&event, &JsValue::from_str("serialNumber"))
                    .ok()
                    .and_then(|value| value.as_string())
                    .unwrap_or_else(|| "unknown".to_string());
                let record_summary = summarize_ndef_records(&event);

                let payload = js_sys::Object::new();
                let _: bool = js_sys::Reflect::set(
                    &payload,
                    &JsValue::from_str("serialNumber"),
                    &JsValue::from_str(&serial_number),
                )
                .unwrap_or(false);
                let _: bool = js_sys::Reflect::set(
                    &payload,
                    &JsValue::from_str("recordSummary"),
                    &JsValue::from_str(&record_summary),
                )
                .unwrap_or(false);
                et_web::ignore(resolve.call1(&JsValue::NULL, &payload));
            });
            let on_reading = Closure::once(on_reading_box);

            let on_reading_error_box: Box<dyn FnOnce(JsValue)> = Box::new(move |event: JsValue| {
                let message = js_sys::Reflect::get(&event, &JsValue::from_str("message"))
                    .ok()
                    .and_then(|value| value.as_string())
                    .unwrap_or_else(|| "NFC reading failed".to_string());
                et_web::ignore(reject_for_error.call1(&JsValue::NULL, &JsValue::from_str(&message)));
            });
            let on_reading_error = Closure::once(on_reading_error_box);

            let _: bool = js_sys::Reflect::set(
                &reader,
                &JsValue::from_str("onreading"),
                on_reading.as_ref().unchecked_ref(),
            )
            .unwrap_or(false);
            let _: bool = js_sys::Reflect::set(
                &reader,
                &JsValue::from_str("onreadingerror"),
                on_reading_error.as_ref().unchecked_ref(),
            )
            .unwrap_or(false);

            on_reading.forget();
            on_reading_error.forget();
            timeout_closure.forget();
        });

        let result = JsFuture::from(promise).await?;
        let serial_number = js_sys::Reflect::get(&result, &JsValue::from_str("serialNumber"))?
            .as_string()
            .unwrap_or_else(|| "unknown".to_string());
        let record_summary = js_sys::Reflect::get(&result, &JsValue::from_str("recordSummary"))?
            .as_string()
            .unwrap_or_else(|| "no records".to_string());

        info!(
            "NFC scan captured: serial_number={} summary={}",
            serial_number, record_summary
        );

        Ok(Self {
            serial_number,
            record_summary,
        })
    }

    #[must_use]
    #[wasm_bindgen(js_name = serialNumber)]
    pub fn serial_number(&self) -> String {
        self.serial_number.clone()
    }

    #[must_use]
    #[wasm_bindgen(js_name = recordSummary)]
    pub fn record_summary(&self) -> String {
        self.record_summary.clone()
    }
}

#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "JS number->u32 array length conversion via f64; no native u32 type"
)]
fn summarize_ndef_records(event: &JsValue) -> String {
    let Ok(message) = js_sys::Reflect::get(event, &JsValue::from_str("message")) else {
        return "no message".to_string();
    };
    let Ok(records) = js_sys::Reflect::get(&message, &JsValue::from_str("records")) else {
        return "no records".to_string();
    };
    let Some(length) = js_sys::Reflect::get(&records, &JsValue::from_str("length"))
        .ok()
        .and_then(|value| value.as_f64())
        .map(|len| len as u32)
    else {
        return "no records".to_string();
    };

    let mut summary = Vec::new();
    for index in 0..length {
        let Ok(record) = js_sys::Reflect::get(&records, &JsValue::from_f64(f64::from(index))) else {
            continue;
        };
        let record_type = js_sys::Reflect::get(&record, &JsValue::from_str("recordType"))
            .ok()
            .and_then(|value| value.as_string())
            .unwrap_or_else(|| "unknown".to_string());
        let media_type = js_sys::Reflect::get(&record, &JsValue::from_str("mediaType"))
            .ok()
            .and_then(|value| value.as_string())
            .unwrap_or_default();
        let id = js_sys::Reflect::get(&record, &JsValue::from_str("id"))
            .ok()
            .and_then(|value| value.as_string())
            .unwrap_or_default();

        let mut parts = vec![format!("type={record_type}")];
        if !media_type.is_empty() {
            parts.push(format!("media={media_type}"));
        }
        if !id.is_empty() {
            parts.push(format!("id={id}"));
        }
        summary.push(parts.join(","));
    }

    if summary.is_empty() {
        "no records".to_string()
    } else {
        summary.join(" | ")
    }
}

#[wasm_bindgen(start)]
pub fn init() {
    et_web::ignore(tracing_wasm::try_set_as_global_default());
    info!("nfc module initialized");
}

#[must_use]
#[wasm_bindgen]
#[expect(
    clippy::missing_const_for_fn,
    reason = "wasm_bindgen rejects const fns; cannot be marked const"
)]
pub fn is_running() -> bool {
    false
}

#[wasm_bindgen]
pub async fn run() -> Result<(), JsValue> {
    set_module_status("nfc: Starting NFC scan...\nPlease tap an NFC tag within 60 seconds.")?;
    log("entered run()");

    let outcome = async {
        let ws_url = websocket_url()?;
        let mut client = WsClient::new(WsClientConfig::new(ws_url));
        client.connect()?;
        wait_for_connected(&client).await?;
        log(&format!("websocket connected with agent_id={}", client.get_agent_id()));

        log("waiting for NFC tap (60 second timeout)...");
        set_module_status("nfc: Waiting for NFC tap...\nPlease hold your device near an NFC tag.")?;

        let result = NfcScanResult::scan_once().await?;
        let serial = result.serial_number();
        let summary = result.record_summary();
        log(&format!("NFC scan captured: serial={serial} summary={summary}"));

        client.send_client_event(
            "nfc",
            "scan_captured",
            json!({
                "serial_number": serial,
                "record_summary": summary,
            }),
        )?;

        set_module_status(&format!("nfc: Scan captured\nSerial: {serial}\nSummary: {summary}"))?;

        client.disconnect();
        Ok(())
    }
    .await;

    if let Err(error) = &outcome {
        let message = describe_js_error(error);
        let error_display = if message.contains("not available") || message.contains("not supported") {
            format!("nfc: Not available\nWeb NFC requires: Chrome/Edge on Android, HTTPS connection\nError: {message}")
        } else if message.contains("timed out") || message.contains("timeout") {
            "nfc: Timeout\n\nNo NFC tag was detected within 60 seconds.\nPlease try again and tap an NFC tag."
                .to_string()
        } else {
            format!("nfc: Error\n\n{message}")
        };
        et_web::ignore(set_module_status(&error_display));
        log(&format!("error: {message}"));
    }

    outcome
}

fn log(message: &str) {
    let line = format!("[nfc] {message}");
    web_sys::console::log_1(&JsValue::from_str(&line));

    if let Some(window) = web_sys::window()
        && let Some(document) = window.document()
        && let Some(log_el) = document.get_element_by_id("log")
    {
        let current = log_el.text_content().unwrap_or_default();
        let next = if current.is_empty() {
            line
        } else {
            format!("{current}\n{line}")
        };
        log_el.set_text_content(Some(&next));
    }
}

fn set_module_status(message: &str) -> Result<(), JsValue> {
    set_textarea_value("module-output", message)
}

fn describe_js_error(error: &JsValue) -> String {
    if let Some(text) = error.as_string() {
        return text;
    }

    if let Some(obj) = error.dyn_ref::<js_sys::Object>()
        && let Ok(message) = js_sys::Reflect::get(obj, &JsValue::from_str("message"))
        && let Some(msg) = message.as_string()
    {
        return msg;
    }

    js_sys::JSON::stringify(error)
        .ok()
        .and_then(|json| json.as_string())
        .unwrap_or_else(|| "Unknown error".to_string())
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

async fn sleep_ms(duration_ms: i32) -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
    let promise = Promise::new(&mut |resolve, reject| {
        let callback = Closure::once_into_js(move || {
            et_web::ignore(resolve.call0(&JsValue::NULL));
        });

        if let Err(error) =
            window.set_timeout_with_callback_and_timeout_and_arguments_0(callback.unchecked_ref(), duration_ms)
        {
            et_web::ignore(reject.call1(&JsValue::NULL, &error));
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
