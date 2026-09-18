#![cfg(test)]
#![cfg(target_arch = "wasm32")]
#![cfg_attr(wasm_bindgen_unstable_test_coverage, feature(coverage_attribute))]

use et_web::{
    SENSOR_PERMISSION_GRANTED, describe_js_error, get_media_devices, request_sensor_permission, sleep_ms, sleep_ms_on,
    websocket_url, websocket_url_from_location,
};
use et_ws_wasm_agent::{WsClient, WsClientConfig, wait_for_connected};
use js_sys::{Function, Object, Reflect};
use wasm_bindgen::{JsCast as _, JsValue};
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
async fn test_websocket_connection() {
    let config = WsClientConfig::new("ws://127.0.0.1:8080/ws".to_string());
    let mut client = WsClient::new(config);

    let result = client.connect();
    assert!(result.is_ok(), "Client should initiate connection without errors");

    wait_for_connected(&client)
        .await
        .expect("client should reach the connected state against the live cov-server");

    assert_eq!(
        client.get_state(),
        "connected",
        "Client should be connected to the server"
    );
}

/// A dead endpoint must exhaust the poll loop rather than report success.
///
/// Nothing listens on 45123, so the client never reaches the connected state and `wait_for_connected` spends its
/// full ~10s budget before giving up. That wait is the point: the timeout arm is the only path returning `Err`.
/// The port is deliberately high rather than something like 1 or 9, which browsers refuse outright as blocked
/// ports -- `new WebSocket()` would throw before a connection was ever attempted, testing the wrong thing.
#[wasm_bindgen_test]
async fn wait_for_connected_times_out_on_a_dead_endpoint() {
    let config = WsClientConfig::new("ws://127.0.0.1:45123/ws".to_string());
    let mut client = WsClient::new(config);
    let _connect = client.connect();

    let outcome = wait_for_connected(&client).await;
    assert!(outcome.is_err(), "a dead endpoint must never report connected");
}

/// `sleep_ms` resolves through `window.setTimeout` rather than hanging or rejecting.
#[wasm_bindgen_test]
async fn sleep_ms_resolves() {
    sleep_ms(10).await.expect("window.setTimeout should resolve the sleep");
}

/// The websocket endpoint is derived from the page's own location.
#[wasm_bindgen_test]
fn websocket_url_derives_the_endpoint_from_the_page() {
    let url = websocket_url().expect("a browser page always has window.location");

    assert!(
        url.starts_with("ws://") || url.starts_with("wss://"),
        "expected a websocket scheme, got {url}"
    );
    assert!(url.ends_with("/ws"), "expected the /ws endpoint path, got {url}");
}

/// A string error is described as itself, without going near `JSON.stringify`.
#[wasm_bindgen_test]
fn describe_js_error_uses_the_string_form() {
    let error = JsValue::from_str("plain string error");

    assert_eq!(describe_js_error(&error), "plain string error");
}

/// A non-string error falls back to `JSON.stringify`.
#[wasm_bindgen_test]
fn describe_js_error_falls_back_to_json() {
    let error = Object::new();
    let _set = Reflect::set(error.as_ref(), &JsValue::from_str("code"), &JsValue::from_f64(7.0))
        .expect("setting a property on a fresh object cannot fail");

    let described = describe_js_error(error.as_ref());

    assert!(
        described.contains("code"),
        "expected the stringified key, got {described}"
    );
    assert!(
        described.contains('7'),
        "expected the stringified value, got {described}"
    );
}

/// A cyclic error makes `JSON.stringify` throw, leaving the `Debug` rendering as the last resort.
#[wasm_bindgen_test]
fn describe_js_error_falls_back_to_debug_when_json_throws() {
    let error = Object::new();
    let _set = Reflect::set(error.as_ref(), &JsValue::from_str("self"), error.as_ref())
        .expect("setting a property on a fresh object cannot fail");

    let described = describe_js_error(error.as_ref());

    assert!(
        !described.is_empty(),
        "the Debug fallback must still describe the error"
    );
}

/// A fresh object carrying one property, standing in for a browser object the helper under test reads.
///
/// The helpers reach their properties through `Reflect::get`, so a plain object with the right key is
/// indistinguishable from the real navigator, window or location -- which is what lets each refusal arm be
/// driven on demand instead of waiting for a browser that happens to lack the feature.
fn object_with(key: &str, value: &JsValue) -> Object {
    let object = Object::new();
    let _set = Reflect::set(object.as_ref(), &JsValue::from_str(key), value)
        .expect("setting a property on a fresh object cannot fail");
    object
}

/// A `location` stand-in with just the two properties `websocket_url_from_location` reads.
fn location_with(protocol: &str, host: &str) -> JsValue {
    let location = object_with("protocol", &JsValue::from_str(protocol));
    let _set = Reflect::set(location.as_ref(), &JsValue::from_str("host"), &JsValue::from_str(host))
        .expect("setting a property on a fresh object cannot fail");
    location.into()
}

/// The endpoint follows the page's scheme: `wss:` behind https, plain `ws:` otherwise.
#[wasm_bindgen_test]
fn websocket_url_upgrades_to_wss_only_on_an_https_page() {
    let secure = websocket_url_from_location(&location_with("https:", "edge.example:8443"))
        .expect("a location with protocol and host yields a URL");
    assert_eq!(secure, "wss://edge.example:8443/ws");

    let plain = websocket_url_from_location(&location_with("http:", "localhost:8080"))
        .expect("a location with protocol and host yields a URL");
    assert_eq!(plain, "ws://localhost:8080/ws");
}

/// A location without a string `protocol` is refused rather than defaulted, so a broken page fails loudly.
#[wasm_bindgen_test]
fn websocket_url_refuses_a_location_without_a_protocol() {
    let err = websocket_url_from_location(&Object::new().into()).unwrap_err();

    assert_eq!(
        err.as_string().as_deref(),
        Some("window.location.protocol is unavailable")
    );
}

/// Each of the three shapes `get_media_devices` refuses: the property missing, null, or not a `MediaDevices`.
///
/// The first two are the insecure-context case (a page served over plain http from a non-local host), where
/// the browser leaves `navigator.mediaDevices` undefined; the third is a navigator whose property exists but
/// is not the real API object, which the cast catches.
#[wasm_bindgen_test]
fn get_media_devices_refuses_a_navigator_without_a_usable_media_devices() {
    let unavailable = "navigator.mediaDevices is unavailable. Use https://... or http://localhost and allow access.";
    let cases = [
        (Object::new(), unavailable),
        (object_with("mediaDevices", &JsValue::NULL), unavailable),
        (
            object_with("mediaDevices", Object::new().as_ref()),
            "navigator.mediaDevices is not accessible in this browser",
        ),
    ];
    for (navigator, expected) in cases {
        let navigator: web_sys::Navigator = navigator.unchecked_into();
        let err = get_media_devices(&navigator).unwrap_err();
        assert_eq!(err.as_string().as_deref(), Some(expected));
    }
}

/// On a secure page the real navigator's `MediaDevices` is handed back as-is.
///
/// The test page is served from a loopback origin, which the browser treats as secure, so the real navigator
/// carries the API object.
#[wasm_bindgen_test]
fn get_media_devices_returns_the_real_api_on_a_secure_page() {
    let navigator = web_sys::window().expect("the test page has a window").navigator();

    let devices = get_media_devices(&navigator).expect("a loopback page is a secure context");
    assert!(devices.is_instance_of::<web_sys::MediaDevices>());
}

/// Every target that cannot be asked is treated as already granted.
///
/// That is no target at all, or one without a callable `requestPermission` -- which is every browser except
/// iOS Safari, where the method exists.
#[wasm_bindgen_test]
async fn request_sensor_permission_is_granted_wherever_nothing_can_be_asked() {
    let targets = [
        JsValue::NULL,
        JsValue::UNDEFINED,
        Object::new().into(),
        object_with("requestPermission", &JsValue::NULL).into(),
    ];
    for target in targets {
        let outcome = request_sensor_permission(target)
            .await
            .expect("an unaskable target must not be an error");
        assert_eq!(outcome, SENSOR_PERMISSION_GRANTED);
    }
}

/// With a callable `requestPermission`, its promise decides the answer.
///
/// A string answer is returned as-is, anything else reads as granted, and a property that is not callable is
/// an error rather than a silent grant.
#[wasm_bindgen_test]
async fn request_sensor_permission_asks_a_target_that_can_answer() {
    let denies = object_with(
        "requestPermission",
        Function::new_no_args("return Promise.resolve('denied');").as_ref(),
    );
    let outcome = request_sensor_permission(denies.into())
        .await
        .expect("a resolving requestPermission is not an error");
    assert_eq!(outcome, "denied");

    let answers_nonsense = object_with(
        "requestPermission",
        Function::new_no_args("return Promise.resolve(42);").as_ref(),
    );
    let outcome = request_sensor_permission(answers_nonsense.into())
        .await
        .expect("a non-string answer falls back to granted rather than failing");
    assert_eq!(outcome, SENSOR_PERMISSION_GRANTED);

    let not_callable = object_with("requestPermission", &JsValue::from_f64(1.0));
    let err = request_sensor_permission(not_callable.into()).await.unwrap_err();
    assert_eq!(err.as_string().as_deref(), Some("requestPermission is not callable"));
}

/// A `setTimeout` that throws turns into a rejection of the sleep, carrying the thrown error.
///
/// The browser's own `setTimeout` never throws for a callback and a delay, so a window stand-in whose
/// `setTimeout` does is the only way to see the rejection arm run.
#[wasm_bindgen_test]
async fn sleep_ms_on_rejects_when_set_timeout_throws() {
    let refusing = object_with(
        "setTimeout",
        Function::new_no_args("throw new Error('no timers here');").as_ref(),
    );
    let window: web_sys::Window = refusing.unchecked_into();

    let err = sleep_ms_on(&window, 1).await.unwrap_err();
    assert!(
        err.is_instance_of::<js_sys::Error>(),
        "the rejection must carry the thrown error, got {}",
        describe_js_error(&err)
    );
}
