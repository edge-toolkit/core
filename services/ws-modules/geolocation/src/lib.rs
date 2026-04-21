use et_ws_wasm_agent::{WsClient, WsClientConfig, set_textarea_value};
use js_sys::{Promise, Reflect};
use serde_json::json;
use tracing::info;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen]
pub struct GeolocationReading {
    latitude: f64,
    longitude: f64,
    accuracy_meters: f64,
}

#[wasm_bindgen]
impl GeolocationReading {
    #[wasm_bindgen(js_name = request)]
    pub async fn request() -> Result<GeolocationReading, JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
        let navigator = window.navigator();
        let geolocation = js_sys::Reflect::get(&navigator, &JsValue::from_str("geolocation"))?;
        if geolocation.is_undefined() || geolocation.is_null() {
            return Err(JsValue::from_str(
                "navigator.geolocation is unavailable. Use https://... or http://localhost and allow access.",
            ));
        }

        let options = js_sys::Object::new();
        js_sys::Reflect::set(&options, &JsValue::from_str("enableHighAccuracy"), &JsValue::TRUE)?;
        js_sys::Reflect::set(&options, &JsValue::from_str("maximumAge"), &JsValue::from_f64(0.0))?;
        js_sys::Reflect::set(&options, &JsValue::from_str("timeout"), &JsValue::from_f64(10_000.0))?;

        let promise = js_sys::Promise::new(&mut |resolve, reject| {
            let reject_for_callback = reject.clone();
            let success = Closure::once(Box::new(move |position: JsValue| {
                let _ = resolve.call1(&JsValue::NULL, &position);
            }) as Box<dyn FnOnce(JsValue)>);

            let failure = Closure::once(Box::new(move |error: JsValue| {
                let _ = reject_for_callback.call1(&JsValue::NULL, &error);
            }) as Box<dyn FnOnce(JsValue)>);

            let get_current_position = js_sys::Reflect::get(&geolocation, &JsValue::from_str("getCurrentPosition"))
                .ok()
                .and_then(|value| value.dyn_into::<js_sys::Function>().ok());

            if let Some(get_current_position) = get_current_position {
                let _ = get_current_position.call3(
                    &geolocation,
                    success.as_ref().unchecked_ref(),
                    failure.as_ref().unchecked_ref(),
                    &options,
                );
            } else {
                let _ = reject.call1(
                    &JsValue::NULL,
                    &JsValue::from_str("navigator.geolocation.getCurrentPosition is not callable"),
                );
            }

            success.forget();
            failure.forget();
        });

        let position = JsFuture::from(promise).await?;
        let coords = js_sys::Reflect::get(&position, &JsValue::from_str("coords"))?;
        let latitude = js_sys::Reflect::get(&coords, &JsValue::from_str("latitude"))?
            .as_f64()
            .ok_or_else(|| JsValue::from_str("Geolocation latitude is missing"))?;
        let longitude = js_sys::Reflect::get(&coords, &JsValue::from_str("longitude"))?
            .as_f64()
            .ok_or_else(|| JsValue::from_str("Geolocation longitude is missing"))?;
        let accuracy_meters = js_sys::Reflect::get(&coords, &JsValue::from_str("accuracy"))?
            .as_f64()
            .ok_or_else(|| JsValue::from_str("Geolocation accuracy is missing"))?;

        info!(
            "Geolocation reading acquired: latitude={} longitude={} accuracy={}m",
            latitude, longitude, accuracy_meters
        );

        Ok(GeolocationReading {
            latitude,
            longitude,
            accuracy_meters,
        })
    }

    pub fn latitude(&self) -> f64 {
        self.latitude
    }

    pub fn longitude(&self) -> f64 {
        self.longitude
    }

    #[wasm_bindgen(js_name = accuracyMeters)]
    pub fn accuracy_meters(&self) -> f64 {
        self.accuracy_meters
    }
}

#[wasm_bindgen(start)]
pub fn init() {
    let _ = tracing_wasm::try_set_as_global_default();
    info!("geolocation module initialized");
}

#[wasm_bindgen]
pub fn is_running() -> bool {
    false
}

#[wasm_bindgen]
pub async fn run() -> Result<(), JsValue> {
    set_module_status("geolocation: entered run()")?;
    log("entered run()")?;

    let outcome = async {
        let ws_url = websocket_url()?;
        let mut client = WsClient::new(WsClientConfig::new(ws_url));
        client.connect()?;
        wait_for_connected(&client).await?;
        log(&format!("websocket connected with agent_id={}", client.get_client_id()))?;

        log("requesting geolocation access")?;
        let reading = GeolocationReading::request().await?;
        let lat = reading.latitude();
        let lon = reading.longitude();
        let acc = reading.accuracy_meters();
        log(&format!("geolocation acquired: lat={} lon={} acc={}m", lat, lon, acc))?;

        client.send_client_event(
            "geolocation",
            "reading_acquired",
            json!({
                "latitude": lat,
                "longitude": lon,
                "accuracy": acc,
            }),
        )?;

        set_module_status(&format!(
            "geolocation: reading acquired\nlat: {}\nlon: {}\nacc: {}m",
            lat, lon, acc
        ))?;

        client.disconnect();
        Ok(())
    }
    .await;

    if let Err(error) = &outcome {
        let message = describe_js_error(error);
        let _ = set_module_status(&format!("geolocation: error\n{}", message));
        let _ = log(&format!("error: {}", message));
    }

    outcome
}

fn log(message: &str) -> Result<(), JsValue> {
    let line = format!("[geolocation] {}", message);
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
