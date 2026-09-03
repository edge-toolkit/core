#![expect(
    clippy::future_not_send,
    clippy::single_call_fn,
    reason = "browser WASM module: JsFuture is !Send; module-local helpers like wait_for_* are single-use by design"
)]

use et_web::{JsFunctionExt as _, JsPromiseExt as _, describe_js_error, websocket_url};
use et_ws_wasm_agent::{WsClient, WsClientConfig, set_textarea_value, wait_for_connected};
use serde_json::json;
use tracing::info;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen]
pub struct BluetoothAccess {
    device: JsValue,
}

#[wasm_bindgen]
impl BluetoothAccess {
    #[wasm_bindgen(js_name = request)]
    pub async fn request() -> Result<Self, JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
        let navigator = window.navigator();
        let bluetooth = js_sys::Reflect::get(&navigator, &JsValue::from_str("bluetooth"))?;
        if bluetooth.is_undefined() || bluetooth.is_null() {
            return Err(JsValue::from_str(
                "Web Bluetooth is not available in this browser context",
            ));
        }

        let options = js_sys::Object::new();
        let _: bool = js_sys::Reflect::set(&options, &JsValue::from_str("acceptAllDevices"), &JsValue::TRUE)?;

        let request_device = js_sys::Reflect::get(&bluetooth, &JsValue::from_str("requestDevice"))?
            .into_function("navigator.bluetooth.requestDevice")?;
        let promise = request_device
            .call1(&bluetooth, &options)?
            .into_promise("requestDevice")?;
        let device = JsFuture::from(promise).await?;

        info!(
            "Bluetooth device selected: {:?}",
            js_sys::Reflect::get(&device, &JsValue::from_str("name"))
                .ok()
                .and_then(|value| value.as_string())
                .unwrap_or_else(|| "unknown".to_string())
        );

        Ok(Self { device })
    }

    #[must_use]
    pub fn id(&self) -> String {
        js_sys::Reflect::get(&self.device, &JsValue::from_str("id"))
            .ok()
            .and_then(|value| value.as_string())
            .unwrap_or_default()
    }

    #[must_use]
    pub fn name(&self) -> String {
        js_sys::Reflect::get(&self.device, &JsValue::from_str("name"))
            .ok()
            .and_then(|value| value.as_string())
            .unwrap_or_else(|| "unknown".to_string())
    }

    #[must_use]
    #[wasm_bindgen(js_name = gattConnected)]
    pub fn gatt_connected(&self) -> bool {
        js_sys::Reflect::get(&self.device, &JsValue::from_str("gatt"))
            .ok()
            .filter(|gatt| !gatt.is_null() && !gatt.is_undefined())
            .and_then(|gatt| js_sys::Reflect::get(&gatt, &JsValue::from_str("connected")).ok())
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
    }

    #[wasm_bindgen(js_name = connectGatt)]
    pub async fn connect_gatt(&self) -> Result<(), JsValue> {
        let gatt = js_sys::Reflect::get(&self.device, &JsValue::from_str("gatt"))?;
        if gatt.is_null() || gatt.is_undefined() {
            return Err(JsValue::from_str("Selected device has no GATT server"));
        }

        let connect =
            js_sys::Reflect::get(&gatt, &JsValue::from_str("connect"))?.into_function("device.gatt.connect")?;
        let promise = connect.call0(&gatt)?.into_promise("device.gatt.connect")?;
        let _server: JsValue = JsFuture::from(promise).await?;
        info!("Connected to Bluetooth GATT server for {}", self.name());
        Ok(())
    }
}

#[wasm_bindgen(start)]
pub fn init() {
    et_web::ignore(tracing_wasm::try_set_as_global_default());
    info!("bluetooth module initialized");
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
    set_module_status("bluetooth: entered run()")?;
    log("entered run()");

    let outcome = async {
        let ws_url = websocket_url()?;
        let mut client = WsClient::new(WsClientConfig::new(ws_url));
        client.connect()?;
        wait_for_connected(&client).await?;
        log(&format!("websocket connected with agent_id={}", client.get_agent_id()));

        log("requesting bluetooth access");
        let access = BluetoothAccess::request().await?;
        let id = access.id();
        let name = access.name();
        log(&format!("bluetooth device selected: {name} ({id})"));

        client.send_client_event(
            "bluetooth",
            "device_selected",
            json!({
                "id": id,
                "name": name,
            }),
        )?;

        set_module_status(&format!("bluetooth: device selected\n{name} ({id})"))?;

        client.disconnect();
        Ok(())
    }
    .await;

    if let Err(error) = &outcome {
        let message = describe_js_error(error);
        et_web::ignore(set_module_status(&format!("bluetooth: error\n{message}")));
        log(&format!("error: {message}"));
    }

    outcome
}

fn log(message: &str) {
    let line = format!("[bluetooth] {message}");
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
