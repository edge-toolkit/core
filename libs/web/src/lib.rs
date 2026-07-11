use wasm_bindgen::prelude::*;

mod error;

pub use self::error::{JsCastExt, JsFunctionExt, JsPromiseExt, JsResultExt};

pub const SENSOR_PERMISSION_GRANTED: &str = "granted";

/// Return this module's raw minicov coverage buffer (a `.profraw`), or empty on failure.
///
/// Present only in the `coverage` build. `wasm-bindgen` collects this export into every dependent browser
/// module's JS glue, so the web-runner can pull each module's coverage after running it -- `wasm32-unknown-unknown`
/// has no filesystem, so the bytes come back through JS rather than a file. The web-runner then routes them
/// through the same llc + llvm-cov pipeline the WASI guests use (see the `wasi-cov` mise task).
#[cfg(feature = "coverage")]
#[wasm_bindgen]
#[expect(
    unsafe_code,
    reason = "minicov::capture_coverage is unsafe; the browser wasm module is single-threaded"
)]
#[must_use]
pub fn __et_capture_coverage() -> Vec<u8> {
    let mut data = Vec::new();
    // SAFETY: single-threaded browser wasm; called once after the module's run() completes.
    match unsafe { minicov::capture_coverage(&mut data) } {
        Ok(()) => data,
        Err(_) => Vec::new(),
    }
}

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
