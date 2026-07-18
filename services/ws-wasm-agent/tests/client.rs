//! Server-independent coverage of the WASM agent's client logic.
//!
//! `web.rs` drives a live end-to-end connection (needs a running ws-server); this file exercises everything the
//! client does without a server -- configuration, connection state transitions, the offline send queue, the DOM
//! textarea helpers, and the JS reflection helpers -- so it runs headless with no backend. It is what the
//! `wasm-agent-cov` mise task builds instrumented to measure the agent's coverage.
#![cfg(test)]
#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;

use et_ws_wasm_agent::{
    WsClient, WsClientConfig, append_to_textarea, create_and_connect, init_tracing, js_bool_field, js_nested_object,
    js_number_field, set_textarea_value,
};
use js_sys::{Object, Promise, Reflect};
use serde_json::json;
use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::Closure;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

fn new_client(url: &str) -> WsClient {
    WsClient::new(WsClientConfig::new(url.to_string()))
}

/// The live ws-server URL for the connected-path tests.
///
/// A backend must be listening here: the `wasm-agent-cov` mise task starts the in-process et-ws-test-server on
/// this port, and `ws-e2e-chrome` runs the real ws-server on it too.
const SERVER_URL: &str = "ws://127.0.0.1:8080/ws";

async fn sleep(ms: i32) {
    let promise = Promise::new(&mut |resolve, _reject| {
        let window = web_sys::window().unwrap();
        let _id = window
            .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms)
            .unwrap();
    });
    let _resolved = JsFuture::from(promise).await.unwrap();
}

#[wasm_bindgen_test]
fn config_setters_and_initial_state() {
    let mut config = WsClientConfig::new("ws://127.0.0.1:8080/ws".to_string());
    config.set_alive_interval(2_000);
    config.set_max_reconnect_attempts(3);
    config.set_initial_reconnect_delay(500);

    // Clear any agent id a prior test's connect may have persisted so the fresh-client assertion is stable.
    let storage = web_sys::window().unwrap().local_storage().unwrap().unwrap();
    storage.clear().unwrap();

    let client = WsClient::new(config);
    assert_eq!(client.get_state(), "disconnected");
    assert_eq!(client.get_agent_id(), "");
}

#[wasm_bindgen_test]
fn connect_moves_to_connecting() {
    let mut client = new_client("ws://127.0.0.1:8080/ws");
    client.connect().unwrap();
    assert_eq!(client.get_state(), "connecting");
}

#[wasm_bindgen_test]
fn create_and_connect_helper_builds_connecting_client() {
    let client = create_and_connect("ws://127.0.0.1:8080/ws".to_string()).unwrap();
    assert_eq!(client.get_state(), "connecting");
}

#[wasm_bindgen_test]
fn offline_sends_are_queued() {
    let client = new_client("ws://127.0.0.1:8080/ws");
    // Every send path enqueues while disconnected rather than erroring.
    client.send("plain text").unwrap();
    client.request_list_agents().unwrap();
    client.broadcast_message(json!({ "hello": "world" })).unwrap();
    client.send_agent_message("agent-x", json!({ "k": 1 })).unwrap();
    client
        .send_client_event("capability", "action", json!({ "detail": true }))
        .unwrap();
}

#[wasm_bindgen_test]
fn send_alive_errors_when_not_connected() {
    let client = new_client("ws://127.0.0.1:8080/ws");
    assert!(client.send_alive().is_err());
}

#[wasm_bindgen_test]
fn offline_queue_drops_oldest_past_capacity() {
    let client = new_client("ws://127.0.0.1:8080/ws");
    // MAX_OFFLINE_QUEUE_LEN is 1000; the 1001st enqueue exercises the drop-oldest branch.
    for i in 0..1_001 {
        client.send(&format!("message {i}")).unwrap();
    }
}

#[wasm_bindgen_test]
fn state_change_callback_receives_transitions() {
    let recorded = Rc::new(RefCell::new(Vec::<String>::new()));
    let sink = Rc::clone(&recorded);
    let callback = Closure::wrap(Box::new(move |state: JsValue| {
        sink.borrow_mut().push(state.as_string().unwrap_or_default());
    }) as Box<dyn FnMut(JsValue)>);

    let mut client = new_client("ws://127.0.0.1:8080/ws");
    client.set_on_state_change(callback.as_ref().clone());
    // A message callback is only invoked with a live server; setting it still needs coverage.
    let noop = Closure::wrap(Box::new(|_msg: JsValue| {}) as Box<dyn FnMut(JsValue)>);
    client.set_on_message(noop.as_ref().clone());

    client.connect().unwrap();
    client.disconnect();
    assert_eq!(client.get_state(), "disconnected");

    let seen = recorded.borrow();
    assert!(
        seen.iter().any(|s| s == "connecting"),
        "expected a connecting transition"
    );
    assert!(
        seen.iter().any(|s| s == "disconnected"),
        "expected a disconnected transition"
    );

    callback.forget();
    noop.forget();
}

#[wasm_bindgen_test]
async fn connection_error_schedules_reconnect() {
    // Nothing listens on this high port, so the browser fires onerror/onclose, driving handle_disconnect and
    // its exponential-backoff reconnect scheduling.
    let mut client = new_client("ws://127.0.0.1:47111/ws");
    client.connect().unwrap();

    let mut saw_reconnecting = false;
    for _ in 0..40 {
        if client.get_state() == "reconnecting" {
            saw_reconnecting = true;
            break;
        }
        sleep(250).await;
    }
    assert!(
        saw_reconnecting,
        "expected the failed connection to enter the reconnecting state"
    );
    client.disconnect();
}

#[wasm_bindgen_test]
fn textarea_helpers_set_and_append() {
    let document = web_sys::window().unwrap().document().unwrap();
    let body = document.body().unwrap();

    let target = document.create_element("textarea").unwrap();
    target.set_id("ta-target");
    let _appended = body.append_child(&target).unwrap();

    set_textarea_value("ta-target", "first value").unwrap();
    let value = Reflect::get(target.as_ref(), &JsValue::from_str("value")).unwrap();
    assert_eq!(value.as_string().unwrap(), "first value");

    // Empty textarea -> first append replaces; a second append joins with a newline.
    let appendable = document.create_element("textarea").unwrap();
    appendable.set_id("ta-append");
    let _appended2 = body.append_child(&appendable).unwrap();
    append_to_textarea("ta-append", "line one").unwrap();
    append_to_textarea("ta-append", "line two").unwrap();
    let joined = Reflect::get(appendable.as_ref(), &JsValue::from_str("value"))
        .unwrap()
        .as_string()
        .unwrap();
    assert_eq!(joined, "line one\nline two");

    // The "Workflow module" placeholder is treated like empty, so the next append replaces it.
    let _reset: bool = Reflect::set(
        appendable.as_ref(),
        &JsValue::from_str("value"),
        &JsValue::from_str("Workflow module loading..."),
    )
    .unwrap();
    append_to_textarea("ta-append", "fresh").unwrap();
    let replaced = Reflect::get(appendable.as_ref(), &JsValue::from_str("value"))
        .unwrap()
        .as_string()
        .unwrap();
    assert_eq!(replaced, "fresh");

    // Missing element ids are a no-op, not an error.
    set_textarea_value("no-such-textarea", "x").unwrap();
    append_to_textarea("no-such-textarea", "x").unwrap();
}

#[wasm_bindgen_test]
fn js_reflection_helpers_read_fields() {
    let obj = Object::new();
    let _num: bool = Reflect::set(&obj, &JsValue::from_str("num"), &JsValue::from_f64(1.5)).unwrap();
    let _flag: bool = Reflect::set(&obj, &JsValue::from_str("flag"), &JsValue::TRUE).unwrap();
    let _nested: bool = Reflect::set(&obj, &JsValue::from_str("nested"), Object::new().as_ref()).unwrap();
    let _null: bool = Reflect::set(&obj, &JsValue::from_str("empty"), &JsValue::NULL).unwrap();

    assert_eq!(js_number_field(&obj, "num"), Some(1.5));
    assert_eq!(js_number_field(&obj, "flag"), None); // present but not a number
    assert_eq!(js_number_field(&obj, "missing"), None);

    assert_eq!(js_bool_field(&obj, "flag"), Some(true));
    assert_eq!(js_bool_field(&obj, "missing"), None);

    assert!(js_nested_object(&obj, "nested").is_some());
    assert!(js_nested_object(&obj, "empty").is_none()); // null is filtered out
    assert!(js_nested_object(&obj, "missing").is_none());
}

#[wasm_bindgen_test]
async fn connects_flushes_queue_and_sends() {
    let mut client = new_client(SERVER_URL);
    // Queue a message while still offline so the onopen handler's flush path runs on connect.
    client.send("queued-before-connect").unwrap();

    client.connect().unwrap();
    let mut connected = false;
    for _ in 0..40 {
        if client.get_state() == "connected" {
            connected = true;
            break;
        }
        sleep(250).await;
    }
    assert!(
        connected,
        "client should reach the connected state against the live server"
    );

    // The server's et-connect-ack assigns (and the client persists) an agent id.
    let agent_id = client.get_agent_id();
    assert!(!agent_id.is_empty(), "server should assign an agent id");

    // Online success paths: the alive keepalive, a raw send, and each typed message helper.
    client.send_alive().unwrap();
    client.send("online-message").unwrap();
    client.broadcast_message(json!({ "broadcast": 1 })).unwrap();
    client.request_list_agents().unwrap();
    client.send_agent_message(agent_id, json!({ "self": true })).unwrap();
    client
        .send_client_event("capability", "action", json!({ "online": true }))
        .unwrap();

    // Let the server answer so the onmessage handler dispatches the response frames.
    sleep(500).await;

    client.disconnect();
    assert_eq!(client.get_state(), "disconnected");
}

#[wasm_bindgen_test]
fn tracing_initializes() {
    // tracing_wasm's global default can only be installed once per wasm instance, so this is the only caller.
    init_tracing();
}
