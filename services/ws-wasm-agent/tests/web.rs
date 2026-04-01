#![cfg(target_arch = "wasm32")]

use et_ws_wasm_agent::{WsClient, WsClientConfig};
use js_sys::Promise;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
async fn test_websocket_connection() {
    let config = WsClientConfig::new("ws://127.0.0.1:8080/ws".to_string());
    let mut client = WsClient::new(config);

    // Connect to server
    let result = client.connect();
    assert!(result.is_ok(), "Client should initiate connection without errors");

    // Give it a second to actually connect
    let promise = Promise::new(&mut |resolve, _| {
        let window = web_sys::window().unwrap();
        window
            .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 1000)
            .unwrap();
    });
    let _ = JsFuture::from(promise).await;

    // Assert connection state is successfully connected or at least it didn't fail
    let state = client.get_state();
    assert_eq!(state, "connected", "Client should be connected to the server after 1s");
}
