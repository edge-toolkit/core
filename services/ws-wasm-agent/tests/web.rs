#![cfg(test)]
#![cfg(target_arch = "wasm32")]

use et_web::{describe_js_error, sleep_ms, websocket_url};
use et_ws_wasm_agent::{WsClient, WsClientConfig, wait_for_connected};
use js_sys::{Object, Reflect};
use wasm_bindgen::JsValue;
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
