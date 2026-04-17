use wasm_bindgen::prelude::*;

pub const SENSOR_PERMISSION_GRANTED: &str = "granted";

pub fn get_media_devices(navigator: &web_sys::Navigator) -> Result<web_sys::MediaDevices, JsValue> {
    let media_devices = js_sys::Reflect::get(navigator, &JsValue::from_str("mediaDevices"))?;

    if media_devices.is_undefined() || media_devices.is_null() {
        return Err(JsValue::from_str(
            "navigator.mediaDevices is unavailable. Use https://... or http://localhost and allow access.",
        ));
    }

    media_devices
        .dyn_into::<web_sys::MediaDevices>()
        .map_err(|_| JsValue::from_str("navigator.mediaDevices is not accessible in this browser"))
}

pub async fn request_sensor_permission(target: JsValue) -> Result<String, JsValue> {
    if target.is_null() || target.is_undefined() {
        return Ok(SENSOR_PERMISSION_GRANTED.to_string());
    }

    let request_permission = js_sys::Reflect::get(&target, &JsValue::from_str("requestPermission"))?;
    if request_permission.is_null() || request_permission.is_undefined() {
        return Ok(SENSOR_PERMISSION_GRANTED.to_string());
    }

    let request_permission = request_permission
        .dyn_into::<js_sys::Function>()
        .map_err(|_| JsValue::from_str("requestPermission is not callable"))?;
    let promise = request_permission
        .call0(&target)?
        .dyn_into::<js_sys::Promise>()
        .map_err(|_| JsValue::from_str("requestPermission did not return a Promise"))?;
    let result = wasm_bindgen_futures::JsFuture::from(promise).await?;
    Ok(result
        .as_string()
        .unwrap_or_else(|| SENSOR_PERMISSION_GRANTED.to_string()))
}
