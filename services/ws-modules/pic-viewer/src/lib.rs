//! pic-viewer: broadcast-driven picture viewer for eye captures stored by other agents.
//!
//! `run()` connects the module's own WebSocket client, then polls a queue of capture announcements: every
//! time another agent stores an eye capture, pyeye1 broadcasts a `pyeye1_capture_stored` payload, the server
//! relays it here inside an `et-agent-message` envelope, and this module fetches the announced storage file
//! and draws it onto the page's output canvas. In the demo scenario this module runs on a different device
//! from pyeye1: the capture device announces each stored image and this viewer shows it moments later.
//!
//! The loop stops cleanly after `IDLE_STOP_POLLS` empty polls without a new picture (each displayed picture
//! resets the idle count), after `MAX_RUNTIME_POLLS` overall, or when the page's stop control calls `stop()`.

#![expect(
    clippy::future_not_send,
    clippy::single_call_fn,
    reason = "browser WASM module: JsFuture is !Send; module-local helpers like wait_for_* are single-use by design"
)]

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use edge_toolkit::ws::ServerMessage;
use et_web::{sleep_ms, websocket_url};
use et_ws_wasm_agent::{WsClient, WsClientConfig, set_textarea_value, wait_for_connected};
use serde_json::json;
use tracing::info;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, HtmlImageElement};

const POLL_INTERVAL_MS: i32 = 100;
// The idle window doubles as the wait for the first picture: generous enough to walk to the capture device
// and start pyeye1 there, and to ride out the gap between two pyeye1 runs (pyeye1 stops itself after 30
// seconds, and its captures arrive at least every 5 seconds while it runs with upload consent granted).
// 600 empty 100ms polls = ~60 seconds idle; 3000 loop iterations = ~5 minutes overall.
const IDLE_STOP_POLLS: u32 = 600;
const MAX_RUNTIME_POLLS: u32 = 3_000;
// The `kind` discriminator pyeye1 puts in its capture-stored broadcast payloads.
const CAPTURE_KIND: &str = "pyeye1_capture_stored";

/// One decoded capture announcement: who stored the image and the storage path to fetch it from.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CaptureNotification {
    pub from_agent_id: String,
    pub filename: String,
    pub url: String,
}

thread_local! {
    /// The running viewer's stop flag; `Some` while `run()` is active, shared with `stop()`.
    static STOP_FLAG: RefCell<Option<Rc<Cell<bool>>>> = const { RefCell::new(None) };
}

#[wasm_bindgen(start)]
pub fn init() {
    tracing_wasm::set_as_global_default();
    info!("pic-viewer workflow module initialized");
}

/// Return whether a viewer run is currently active (the page's run button toggles run/stop on this).
#[must_use]
#[wasm_bindgen]
pub fn is_running() -> bool {
    STOP_FLAG.with(|flag| flag.borrow().is_some())
}

/// Request the active viewer loop to stop; it exits at its next poll and cleans up after itself.
#[wasm_bindgen]
pub fn stop() {
    STOP_FLAG.with(|flag| {
        if let Some(stop_requested) = flag.borrow_mut().take() {
            stop_requested.set(true);
        }
    });
    log("stop requested");
}

/// Run the picture-viewer workflow: connect, then display every capture announced by other agents.
#[wasm_bindgen]
pub async fn run() -> Result<(), JsValue> {
    if is_running() {
        return Ok(());
    }
    let stop_requested = Rc::new(Cell::new(false));
    STOP_FLAG.with(|flag| *flag.borrow_mut() = Some(Rc::clone(&stop_requested)));

    let outcome = view_workflow(&stop_requested).await;

    STOP_FLAG.with(|flag| {
        let _active = flag.borrow_mut().take();
    });
    hide_canvas();
    outcome
}

/// Connect the WebSocket client and poll announced captures until an idle/runtime limit or `stop()`.
async fn view_workflow(stop_requested: &Rc<Cell<bool>>) -> Result<(), JsValue> {
    log("entered run()");
    set_module_status("pic-viewer: connecting")?;

    let ws_url = websocket_url()?;
    let mut config = WsClientConfig::new(ws_url);
    // The viewer needs its own agent identity. Clients on one origin share the retained agent id in
    // localStorage, and the server keeps a single session per id -- with a shared id, the page's client and
    // this viewer would steal each other's registration on every (re)connect, and broadcast delivery (which
    // goes only to the currently registered session) would silently flap away from the viewer.
    config.set_use_retained_agent_id(false);
    let mut client = WsClient::new(config);

    let pending: Rc<RefCell<VecDeque<CaptureNotification>>> = Rc::new(RefCell::new(VecDeque::new()));
    let on_message_boxed: Box<dyn FnMut(JsValue)> = Box::new({
        let pending = Rc::clone(&pending);
        move |value: JsValue| queue_capture_notification(&pending, &value)
    });
    let on_message = Closure::wrap(on_message_boxed);
    client.set_on_message(on_message.as_ref().clone());

    client.connect()?;
    wait_for_connected(&client).await?;
    let agent_id = wait_for_agent_id(&client).await?;
    log(&format!("websocket connected with agent_id={agent_id}"));
    set_module_status("pic-viewer: waiting for pictures from other agents...")?;

    let mut shown: u32 = 0;
    let mut idle_polls: u32 = 0;
    let mut total_polls: u32 = 0;

    while !stop_requested.get() {
        if total_polls >= MAX_RUNTIME_POLLS {
            log("viewer finished automatically after ~5 minutes");
            break;
        }
        if idle_polls >= IDLE_STOP_POLLS {
            log("viewer stopped after ~60 seconds without a new picture");
            break;
        }
        total_polls = total_polls.saturating_add(1);

        let next = pending.borrow_mut().pop_front();
        let Some(notification) = next else {
            idle_polls = idle_polls.saturating_add(1);
            sleep_ms(POLL_INTERVAL_MS).await?;
            continue;
        };

        if let Err(error) = show_image(&notification.url).await {
            log(&format!("failed to display {}: {error:?}", notification.url));
            continue;
        }
        shown = shown.saturating_add(1);
        idle_polls = 0;
        set_module_status(&format!(
            "pic-viewer\npictures shown: {shown}\nshowing: {}\nfrom agent: {}",
            notification.filename, notification.from_agent_id
        ))?;
        client.send_client_event(
            "pic_viewer",
            "displayed",
            json!({
                "filename": notification.filename,
                "from_agent_id": notification.from_agent_id,
                "pictures_shown": shown,
                "url": notification.url,
            }),
        )?;
        log(&format!(
            "displayed {} from agent {}",
            notification.url, notification.from_agent_id
        ));
    }

    client.disconnect();
    let message = format!("pic-viewer stopped after showing {shown} picture(s).");
    log(&message);
    set_module_status(&message)?;
    Ok(())
}

/// Queue the capture announcement carried by one incoming frame, ignoring all other traffic.
fn queue_capture_notification(pending: &Rc<RefCell<VecDeque<CaptureNotification>>>, value: &JsValue) {
    let Some(data) = value.as_string() else {
        return;
    };
    if let Some(notification) = parse_capture_notification(&data) {
        pending.borrow_mut().push_back(notification);
    }
}

/// Decode one raw WebSocket frame into a capture announcement, or `None` for any other traffic.
///
/// The socket also carries connect acks, alive responses, and unrelated broadcasts; only a well-formed
/// `et-agent-message` whose payload is a `pyeye1_capture_stored` announcement decodes. The image URL is only
/// accepted as a same-origin `/storage/` path -- announcements arrive from arbitrary peers, and this keeps a
/// buggy or malicious peer from steering the viewer at an external URL.
#[must_use]
pub fn parse_capture_notification(data: &str) -> Option<CaptureNotification> {
    let Ok(ServerMessage::AgentMessage {
        from_agent_id, message, ..
    }) = serde_json::from_str::<ServerMessage>(data)
    else {
        return None;
    };
    if message.get("kind").and_then(serde_json::Value::as_str) != Some(CAPTURE_KIND) {
        return None;
    }
    let url = message.get("url").and_then(serde_json::Value::as_str)?;
    if !url.starts_with("/storage/") {
        return None;
    }
    let filename = message
        .get("filename")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    Some(CaptureNotification {
        from_agent_id,
        filename,
        url: url.to_string(),
    })
}

/// Fetch one announced capture via the browser's own image loader and draw it onto the output canvas.
///
/// The canvas is zoomed to the page width -- the same presentation as pyeye1's cropped view, so both devices
/// in the demo show comparable output.
pub async fn show_image(url: &str) -> Result<(), JsValue> {
    let image = HtmlImageElement::new()?;
    image.set_src(url);
    // decode() resolves once the image is fetched and ready to draw, and rejects on a failed fetch/decode.
    let _decoded = JsFuture::from(image.decode()).await?;

    let canvas = output_canvas()?;
    canvas.set_hidden(false);
    canvas.set_attribute("style", "width: 100%; height: auto;")?;
    canvas.set_width(image.natural_width());
    canvas.set_height(image.natural_height());
    let context = canvas
        .get_context("2d")?
        .ok_or_else(|| JsValue::from_str("2d canvas context unavailable"))?
        .dyn_into::<CanvasRenderingContext2d>()?;
    context.draw_image_with_html_image_element(&image, 0.0, 0.0)
}

/// Look up the page's shared output canvas (the same element pyeye1 renders its cropped view onto).
fn output_canvas() -> Result<HtmlCanvasElement, JsValue> {
    let element = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id("video-output-canvas"))
        .ok_or_else(|| JsValue::from_str("Missing #video-output-canvas element"))?;
    let Ok(canvas) = element.dyn_into::<HtmlCanvasElement>() else {
        return Err(JsValue::from_str("#video-output-canvas is not a canvas"));
    };
    Ok(canvas)
}

/// Hide the output canvas again once the viewer stops (best-effort; a missing element is fine).
fn hide_canvas() {
    if let Ok(canvas) = output_canvas() {
        canvas.set_hidden(true);
    }
}

/// Log one line to the browser console with the module prefix.
fn log(message: &str) {
    let line = format!("[pic-viewer] {message}");
    web_sys::console::log_1(&JsValue::from_str(&line));
}

/// Replace the page's module-output textarea with the viewer's current status.
fn set_module_status(message: &str) -> Result<(), JsValue> {
    set_textarea_value("module-output", message)
}

/// Wait until the server has acknowledged the connection with an agent id, or time out after ~10 seconds.
async fn wait_for_agent_id(client: &WsClient) -> Result<String, JsValue> {
    for _attempt in 0_u32..100 {
        let agent_id = client.get_agent_id();
        if !agent_id.is_empty() {
            return Ok(agent_id);
        }
        sleep_ms(100).await?;
    }

    Err(JsValue::from_str("Timed out waiting for assigned agent_id"))
}
