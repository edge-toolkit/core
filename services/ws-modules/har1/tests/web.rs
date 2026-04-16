#![cfg(target_arch = "wasm32")]
use et_ws_har1::{init, run};
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn init_can_be_called_more_than_once() {
    init();
    init();
}

#[wasm_bindgen_test]
async fn run_reports_environment_error_in_headless_browser() {
    let result = run().await;

    assert!(result.is_err());
}
