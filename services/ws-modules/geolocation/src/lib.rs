#![expect(
    clippy::future_not_send,
    clippy::single_call_fn,
    reason = "browser WASM module: JsFuture is !Send; module-local helpers like wait_for_* are single-use by design"
)]

use et_web::{JsFunctionExt as _, describe_js_error, websocket_url};
use et_ws_wasm_agent::{WsClient, WsClientConfig, set_textarea_value, wait_for_connected};
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
#[expect(
    clippy::missing_const_for_fn,
    reason = "wasm_bindgen rejects const fns; methods cannot be marked const"
)]
impl GeolocationReading {
    #[wasm_bindgen(js_name = request)]
    pub async fn request() -> Result<Self, JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
        let navigator = window.navigator();
        let geolocation = js_sys::Reflect::get(&navigator, &JsValue::from_str("geolocation"))?;
        if geolocation.is_undefined() || geolocation.is_null() {
            return Err(JsValue::from_str(
                "navigator.geolocation is unavailable. Use https://... or http://localhost and allow access.",
            ));
        }

        let options = js_sys::Object::new();
        let _: bool = js_sys::Reflect::set(&options, &JsValue::from_str("enableHighAccuracy"), &JsValue::TRUE)?;
        let _: bool = js_sys::Reflect::set(&options, &JsValue::from_str("maximumAge"), &JsValue::from_f64(0.0))?;
        let _: bool = js_sys::Reflect::set(&options, &JsValue::from_str("timeout"), &JsValue::from_f64(10_000.0))?;

        let promise = js_sys::Promise::new(&mut |resolve, reject| {
            let reject_for_callback = reject.clone();
            let success_box: Box<dyn FnOnce(JsValue)> = Box::new(move |position: JsValue| {
                et_web::ignore(resolve.call1(&JsValue::NULL, &position));
            });
            let success = Closure::once(success_box);

            let failure_box: Box<dyn FnOnce(JsValue)> = Box::new(move |error: JsValue| {
                et_web::ignore(reject_for_callback.call1(&JsValue::NULL, &error));
            });
            let failure = Closure::once(failure_box);

            match js_sys::Reflect::get(&geolocation, &JsValue::from_str("getCurrentPosition"))
                .and_then(|value| value.into_function("navigator.geolocation.getCurrentPosition"))
            {
                Ok(get_current_position) => {
                    et_web::ignore(get_current_position.call3(
                        &geolocation,
                        success.as_ref().unchecked_ref(),
                        failure.as_ref().unchecked_ref(),
                        &options,
                    ));
                }
                Err(err) => {
                    et_web::ignore(reject.call1(&JsValue::NULL, &err));
                }
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

        Ok(Self {
            latitude,
            longitude,
            accuracy_meters,
        })
    }

    #[must_use]
    pub fn latitude(&self) -> f64 {
        self.latitude
    }

    #[must_use]
    pub fn longitude(&self) -> f64 {
        self.longitude
    }

    #[must_use]
    #[wasm_bindgen(js_name = accuracyMeters)]
    pub fn accuracy_meters(&self) -> f64 {
        self.accuracy_meters
    }
}

#[wasm_bindgen(start)]
pub fn init() {
    et_web::ignore(tracing_wasm::try_set_as_global_default());
    info!("geolocation module initialized");
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
    set_module_status("geolocation: entered run()")?;
    log("entered run()");

    let outcome = async {
        let ws_url = websocket_url()?;
        let mut client = WsClient::new(WsClientConfig::new(ws_url));
        client.connect()?;
        wait_for_connected(&client).await?;
        log(&format!("websocket connected with agent_id={}", client.get_agent_id()));

        log("requesting geolocation access");
        let reading = GeolocationReading::request().await?;
        let lat = reading.latitude();
        let lon = reading.longitude();
        let acc = reading.accuracy_meters();
        log(&format!("geolocation acquired: lat={lat} lon={lon} acc={acc}m"));

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
            "geolocation: reading acquired\nlat: {lat}\nlon: {lon}\nacc: {acc}m"
        ))?;

        client.disconnect();
        Ok(())
    }
    .await;

    if let Err(error) = &outcome {
        let message = describe_js_error(error);
        et_web::ignore(set_module_status(&format!("geolocation: error\n{message}")));
        log(&format!("error: {message}"));
    }

    outcome
}

fn log(message: &str) {
    let line = format!("[geolocation] {message}");
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
