use et_ws_wasm_agent::{WsClient, WsClientConfig, set_textarea_value};
use js_sys::{Promise, Reflect};
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
    pub async fn request() -> Result<BluetoothAccess, JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
        let navigator = window.navigator();
        let bluetooth = js_sys::Reflect::get(&navigator, &JsValue::from_str("bluetooth"))?;
        if bluetooth.is_undefined() || bluetooth.is_null() {
            return Err(JsValue::from_str(
                "Web Bluetooth is not available in this browser context",
            ));
        }

        let options = js_sys::Object::new();
        js_sys::Reflect::set(&options, &JsValue::from_str("acceptAllDevices"), &JsValue::TRUE)?;

        let request_device = js_sys::Reflect::get(&bluetooth, &JsValue::from_str("requestDevice"))?
            .dyn_into::<js_sys::Function>()
            .map_err(|_| JsValue::from_str("navigator.bluetooth.requestDevice is not callable"))?;
        let promise = request_device
            .call1(&bluetooth, &options)?
            .dyn_into::<js_sys::Promise>()
            .map_err(|_| JsValue::from_str("requestDevice did not return a Promise"))?;
        let device = JsFuture::from(promise).await?;

        info!(
            "Bluetooth device selected: {:?}",
            js_sys::Reflect::get(&device, &JsValue::from_str("name"))
                .ok()
                .and_then(|value| value.as_string())
                .unwrap_or_else(|| "unknown".to_string())
        );

        Ok(BluetoothAccess { device })
    }

    pub fn id(&self) -> String {
        js_sys::Reflect::get(&self.device, &JsValue::from_str("id"))
            .ok()
            .and_then(|value| value.as_string())
            .unwrap_or_default()
    }

    pub fn name(&self) -> String {
        js_sys::Reflect::get(&self.device, &JsValue::from_str("name"))
            .ok()
            .and_then(|value| value.as_string())
            .unwrap_or_else(|| "unknown".to_string())
    }

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

        let connect = js_sys::Reflect::get(&gatt, &JsValue::from_str("connect"))?
            .dyn_into::<js_sys::Function>()
            .map_err(|_| JsValue::from_str("device.gatt.connect is not callable"))?;
        let promise = connect
            .call0(&gatt)?
            .dyn_into::<js_sys::Promise>()
            .map_err(|_| JsValue::from_str("device.gatt.connect did not return a Promise"))?;
        let _server = JsFuture::from(promise).await?;
        info!("Connected to Bluetooth GATT server for {}", self.name());
        Ok(())
    }
}

#[wasm_bindgen(start)]
pub fn init() {
    let _ = tracing_wasm::try_set_as_global_default();
    info!("bluetooth module initialized");
}

#[wasm_bindgen]
pub fn metadata() -> JsValue {
    serde_wasm_bindgen::to_value(&json!({
        "name": env!("CARGO_PKG_NAME"),
        "description": env!("CARGO_PKG_DESCRIPTION"),
        "version": env!("CARGO_PKG_VERSION"),
    }))
    .unwrap_or(JsValue::NULL)
}

#[wasm_bindgen]
pub fn is_running() -> bool {
    false
}

#[wasm_bindgen]
pub async fn run() -> Result<(), JsValue> {
    set_module_status("bluetooth: entered run()")?;
    log("entered run()")?;

    let outcome = async {
        let ws_url = websocket_url()?;
        let mut client = WsClient::new(WsClientConfig::new(ws_url));
        client.connect()?;
        wait_for_connected(&client).await?;
        log(&format!("websocket connected with agent_id={}", client.get_client_id()))?;

        log("requesting bluetooth access")?;
        let access = BluetoothAccess::request().await?;
        let id = access.id();
        let name = access.name();
        log(&format!("bluetooth device selected: {} ({})", name, id))?;

        client.send_client_event(
            "bluetooth",
            "device_selected",
            json!({
                "id": id,
                "name": name,
            }),
        )?;

        set_module_status(&format!("bluetooth: device selected\n{} ({})", name, id))?;

        client.disconnect();
        Ok(())
    }
    .await;

    if let Err(error) = &outcome {
        let message = describe_js_error(error);
        let _ = set_module_status(&format!("bluetooth: error\n{}", message));
        let _ = log(&format!("error: {}", message));
    }

    outcome
}

fn log(message: &str) -> Result<(), JsValue> {
    let line = format!("[bluetooth] {}", message);
    web_sys::console::log_1(&JsValue::from_str(&line));

    if let Some(window) = web_sys::window()
        && let Some(document) = window.document()
        && let Some(log_el) = document.get_element_by_id("log")
    {
        let current = log_el.text_content().unwrap_or_default();
        let next = if current.is_empty() {
            line
        } else {
            format!("{}\n{}", current, line)
        };
        log_el.set_text_content(Some(&next));
    }

    Ok(())
}

fn set_module_status(message: &str) -> Result<(), JsValue> {
    set_textarea_value("module-output", message)
}

fn describe_js_error(error: &JsValue) -> String {
    error
        .as_string()
        .or_else(|| js_sys::JSON::stringify(error).ok().map(String::from))
        .unwrap_or_else(|| format!("{:?}", error))
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
    Ok(format!("{}//{}/ws", ws_protocol, host))
}
