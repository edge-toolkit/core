//! Headless-browser tests proving `show_image` really renders an image onto the output canvas.
//!
//! Runs under `wasm-pack test --headless` (the `test-pic-viewer-firefox` / `-chrome` mise tasks) in a real
//! browser, exercising the genuine display path: the browser's own image fetch + decode, then the canvas
//! draw. The test data is the repo's favicon, embedded at compile time and handed to the image loader as a
//! same-origin Blob object-URL, so the drawn pixels can be read back without tainting the canvas.

#![cfg(test)]
#![cfg(target_arch = "wasm32")]

use et_ws_pic_viewer::show_image;
use wasm_bindgen::JsCast;
use wasm_bindgen_test::wasm_bindgen_test;

wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// The ws-server page favicon: a real 64x64 PNG, staged into `OUT_DIR` by build.rs and embedded here.
const FAVICON_PNG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/favicon.png"));
const FAVICON_WIDTH: u32 = 64;
const FAVICON_HEIGHT: u32 = 64;

/// Insert the page's shared output canvas (`show_image` looks it up by this id), hidden like the real page.
fn install_output_canvas() -> web_sys::HtmlCanvasElement {
    let document = web_sys::window().unwrap().document().unwrap();
    if let Some(stale) = document.get_element_by_id("video-output-canvas") {
        stale.remove();
    }
    let canvas = document
        .create_element("canvas")
        .unwrap()
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .unwrap();
    canvas.set_id("video-output-canvas");
    canvas.set_hidden(true);
    let _appended = document.body().unwrap().append_child(&canvas).unwrap();
    canvas
}

/// Serve the embedded favicon bytes as a same-origin Blob object-URL the image loader can fetch.
fn favicon_object_url() -> String {
    let bytes = js_sys::Uint8Array::from(FAVICON_PNG);
    let parts = js_sys::Array::of1(&bytes);
    let blob = web_sys::Blob::new_with_u8_array_sequence(&parts).unwrap();
    web_sys::Url::create_object_url_with_blob(&blob).unwrap()
}

#[wasm_bindgen_test]
async fn shows_the_favicon_on_the_output_canvas() {
    let canvas = install_output_canvas();
    let url = favicon_object_url();

    show_image(&url).await.unwrap();
    web_sys::Url::revoke_object_url(&url).unwrap();

    // The canvas must be unhidden and sized to the image's natural dimensions...
    assert!(!canvas.hidden(), "canvas should be unhidden after a successful display");
    assert_eq!(canvas.width(), FAVICON_WIDTH);
    assert_eq!(canvas.height(), FAVICON_HEIGHT);

    // ...and actually carry the drawn pixels: the favicon is an opaque RGB PNG, so every pixel of a real
    // draw has full alpha, while an untouched canvas is all-transparent (alpha 0).
    let context = canvas
        .get_context("2d")
        .unwrap()
        .unwrap()
        .dyn_into::<web_sys::CanvasRenderingContext2d>()
        .unwrap();
    let center = context.get_image_data(32.0, 32.0, 1.0, 1.0).unwrap();
    let pixel = center.data();
    assert_eq!(pixel[3], 255, "center pixel should be opaque after drawing the favicon");
}

#[wasm_bindgen_test]
async fn a_failed_image_load_reports_an_error_and_leaves_the_canvas_hidden() {
    let canvas = install_output_canvas();

    // A revoked object URL can no longer be fetched, so the image decode rejects.
    let url = favicon_object_url();
    web_sys::Url::revoke_object_url(&url).unwrap();

    let outcome = show_image(&url).await;
    assert!(
        outcome.is_err(),
        "a dead URL must surface as an error, not a silent no-op"
    );
    assert!(canvas.hidden(), "the canvas must stay hidden when nothing was drawn");
}
