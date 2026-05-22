use wasm_bindgen::prelude::*;

mod error;

pub use self::error::{JsCastExt, JsFunctionExt, JsPromiseExt, JsResultExt};

pub const SENSOR_PERMISSION_GRANTED: &str = "granted";

pub fn get_media_devices(navigator: &web_sys::Navigator) -> Result<web_sys::MediaDevices, JsValue> {
    let media_devices = js_sys::Reflect::get(navigator, &JsValue::from_str("mediaDevices"))?;

    if media_devices.is_undefined() || media_devices.is_null() {
        return Err(JsValue::from_str(
            "navigator.mediaDevices is unavailable. Use https://... or http://localhost and allow access.",
        ));
    }

    media_devices.dyn_into_msg("navigator.mediaDevices is not accessible in this browser")
}

#[expect(
    clippy::future_not_send,
    reason = "wasm_bindgen_futures::JsFuture is Rc-backed and never Send; runs in single-threaded browser WASM"
)]
pub async fn request_sensor_permission(target: JsValue) -> Result<String, JsValue> {
    if target.is_null() || target.is_undefined() {
        return Ok(SENSOR_PERMISSION_GRANTED.to_string());
    }

    let request_permission = js_sys::Reflect::get(&target, &JsValue::from_str("requestPermission"))?;
    if request_permission.is_null() || request_permission.is_undefined() {
        return Ok(SENSOR_PERMISSION_GRANTED.to_string());
    }

    let request_permission = request_permission.into_function("requestPermission")?;
    let promise = request_permission.call0(&target)?.into_promise("requestPermission")?;
    let result = wasm_bindgen_futures::JsFuture::from(promise).await?;
    Ok(result
        .as_string()
        .unwrap_or_else(|| SENSOR_PERMISSION_GRANTED.to_string()))
}
