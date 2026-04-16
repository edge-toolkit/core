#![cfg(target_arch = "wasm32")]
use et_ws_face_detection::{init, is_running, run, stop};
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn init_can_be_called_more_than_once() {
    init();
    init();
}

#[wasm_bindgen_test]
fn stop_is_idempotent_when_runtime_has_not_started() {
    assert!(!is_running());
    stop().expect("stop should succeed when face detection is not running");
    assert!(!is_running());
}

#[wasm_bindgen_test]
async fn run_failure_leaves_runtime_stopped() {
    let result = run().await;

    match result {
        Ok(()) => {
            assert!(is_running());
            stop().expect("stop should succeed after a successful run");
            assert!(!is_running());
        }
        Err(_) => {
            assert!(!is_running());
        }
    }
}
