use wasm_bindgen::prelude::*;

mod error;

pub use self::error::{JsCastExt, JsFunctionExt, JsPromiseExt, JsResultExt};

pub const SENSOR_PERMISSION_GRANTED: &str = "granted";

/// Discard `value`, marking a `Result` (or other `#[must_use]`) as intentionally ignored.
///
/// The workspace denies `let_underscore*` and `unused_results`, and `DeepSource`'s RS-E1021 flags `drop()` on a
/// non-`Drop` type (e.g. `Result`), so neither `let _ = expr` nor `drop(expr)` is available for discarding one.
/// Passing the value here consumes it -- satisfying `must_use` / `unused_results` -- via neither. Intended for
/// best-effort JS DOM calls in `()`-returning closures and event handlers where the error is deliberately dropped.
pub fn ignore<T>(_value: T) {}

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

/// Resolve after `duration_ms` milliseconds via `window.setTimeout`.
#[expect(
    clippy::future_not_send,
    reason = "wasm_bindgen_futures::JsFuture is Rc-backed and never Send; runs in single-threaded browser WASM"
)]
pub async fn sleep_ms(duration_ms: i32) -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
    let promise = js_sys::Promise::new(&mut |resolve, reject| {
        let callback = Closure::once_into_js(move || {
            ignore(resolve.call0(&JsValue::NULL));
        });

        if let Err(error) =
            window.set_timeout_with_callback_and_timeout_and_arguments_0(callback.unchecked_ref(), duration_ms)
        {
            ignore(reject.call1(&JsValue::NULL, &error));
        }
    });
    wasm_bindgen_futures::JsFuture::from(promise).await.map(ignore)
}

/// Build this page's `/ws` endpoint URL from `window.location`, upgrading to `wss:` on an https page.
pub fn websocket_url() -> Result<String, JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
    let location = js_sys::Reflect::get(window.as_ref(), &JsValue::from_str("location"))?;
    let protocol = js_sys::Reflect::get(&location, &JsValue::from_str("protocol"))?
        .as_string()
        .ok_or_else(|| JsValue::from_str("window.location.protocol is unavailable"))?;
    let host = js_sys::Reflect::get(&location, &JsValue::from_str("host"))?
        .as_string()
        .ok_or_else(|| JsValue::from_str("window.location.host is unavailable"))?;
    let ws_protocol = if protocol == "https:" { "wss:" } else { "ws:" };
    Ok(format!("{ws_protocol}//{host}/ws"))
}

/// Render a `JsValue` error as a displayable string, falling back to `JSON.stringify` then `Debug`.
#[must_use]
pub fn describe_js_error(error: &JsValue) -> String {
    error
        .as_string()
        .or_else(|| js_sys::JSON::stringify(error).ok().map(String::from))
        .unwrap_or_else(|| format!("{error:?}"))
}
