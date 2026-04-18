use std::cell::RefCell;

use et_web::get_media_devices;
use et_ws_wasm_agent::{WsClient, WsClientConfig, set_textarea_value};
use js_sys::{Promise, Reflect};
use serde_json::json;
use tracing::info;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{MediaStream, MediaStreamConstraints};

#[wasm_bindgen]
pub struct VideoCapture {
    stream: MediaStream,
}

#[wasm_bindgen]
impl VideoCapture {
    #[wasm_bindgen(js_name = request)]
    pub async fn request() -> Result<VideoCapture, JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
        let media_devices = get_media_devices(&window.navigator())?;

        let constraints = MediaStreamConstraints::new();
        constraints.set_audio(&JsValue::FALSE);
        constraints.set_video(&JsValue::TRUE);

        let promise = media_devices.get_user_media_with_constraints(&constraints)?;
        let stream = JsFuture::from(promise).await?;
        let stream: MediaStream = stream
            .dyn_into()
            .map_err(|_| JsValue::from_str("getUserMedia did not return a MediaStream"))?;

        info!(
            "Video capture granted with {} video track(s)",
            stream.get_video_tracks().length()
        );

        Ok(VideoCapture { stream })
    }

    #[wasm_bindgen(js_name = trackCount)]
    pub fn track_count(&self) -> u32 {
        self.stream.get_video_tracks().length()
    }

    #[wasm_bindgen(js_name = rawStream)]
    pub fn raw_stream(&self) -> JsValue {
        self.stream.clone().into()
    }

    pub fn stop(&self) {
        let tracks = self.stream.get_tracks();
        for index in 0..tracks.length() {
            if let Some(track) = tracks.get(index).dyn_ref::<web_sys::MediaStreamTrack>() {
                track.stop();
            }
        }
        info!("Video capture tracks stopped");
    }
}

struct VideoCaptureRuntime {
    client: WsClient,
    capture: VideoCapture,
}

thread_local! {
    static VIDEO_CAPTURE_RUNTIME: RefCell<Option<VideoCaptureRuntime>> = const { RefCell::new(None) };
}

#[wasm_bindgen(start)]
pub fn init() {
    let _ = tracing_wasm::try_set_as_global_default();
    info!("video-capture module initialized");
}

#[wasm_bindgen]
pub fn metadata() -> JsValue {
    serde_wasm_bindgen::to_value(&json!({
        "name": env!("CARGO_PKG_NAME"),
        "description": env!("CARGO_PKG_DESCRIPTION"),
        "version": env!("CARGO_PKG_VERSION"),
    }))
    .unwrap_or(JsValue::NULL)
}

#[wasm_bindgen]
pub fn is_running() -> bool {
    VIDEO_CAPTURE_RUNTIME.with(|runtime| runtime.borrow().is_some())
}

#[wasm_bindgen]
pub async fn run() -> Result<(), JsValue> {
    if is_running() {
        return Ok(());
    }

    set_module_status("video-capture: entered run()")?;
    log("entered run()")?;

    let outcome = async {
        let ws_url = websocket_url()?;
        let mut client = WsClient::new(WsClientConfig::new(ws_url));
        client.connect()?;
        wait_for_connected(&client).await?;
        log(&format!("websocket connected with agent_id={}", client.get_client_id()))?;

        log("requesting video capture access")?;
        let capture = VideoCapture::request().await?;
        let tracks = capture.track_count();
        log(&format!("video capture granted: {} tracks", tracks))?;

        // Set up preview
        if let Some(window) = web_sys::window()
            && let Some(document) = window.document()
            && let Some(preview_el) = document.get_element_by_id("video-preview")
        {
            Reflect::set(&preview_el, &JsValue::from_str("srcObject"), &capture.raw_stream())?;
            if let Some(html_el) = preview_el.dyn_ref::<web_sys::HtmlElement>() {
                html_el.style().set_property("display", "block")?;
            }
        }

        client.send_client_event(
            "video",
            "access_granted",
            json!({
                "track_count": tracks,
            }),
        )?;

        set_module_status("video-capture: running")?;

        VIDEO_CAPTURE_RUNTIME.with(|runtime| {
            runtime.borrow_mut().replace(VideoCaptureRuntime { client, capture });
        });

        let stop_callback = Closure::once_into_js(move || {
            if is_running() {
                let _ = log("workflow finished automatically after 10 seconds");
                let _ = stop();
            }
        });
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
        window.set_timeout_with_callback_and_timeout_and_arguments_0(stop_callback.unchecked_ref(), 10000)?;

        Ok(())
    }
    .await;

    if let Err(error) = &outcome {
        let message = describe_js_error(error);
        let _ = set_module_status(&format!("video-capture: error\n{}", message));
        let _ = log(&format!("error: {}", message));
    }

    outcome
}

#[wasm_bindgen]
pub fn stop() -> Result<(), JsValue> {
    VIDEO_CAPTURE_RUNTIME.with(|runtime| {
        if let Some(mut runtime) = runtime.borrow_mut().take() {
            runtime.capture.stop();
            runtime.client.disconnect();

            // Hide preview
            if let Some(window) = web_sys::window()
                && let Some(document) = window.document()
                && let Some(preview_el) = document.get_element_by_id("video-preview")
            {
                Reflect::set(&preview_el, &JsValue::from_str("srcObject"), &JsValue::NULL)?;
                if let Some(html_el) = preview_el.dyn_ref::<web_sys::HtmlElement>() {
                    html_el.style().set_property("display", "none")?;
                }
            }

            log("video-capture stopped")?;
        }
        Ok::<(), JsValue>(())
    })?;

    set_module_status("video-capture: stopped")?;
    Ok(())
}

fn log(message: &str) -> Result<(), JsValue> {
    let line = format!("[video-capture] {}", message);
    web_sys::console::log_1(&JsValue::from_str(&line));

    if let Some(window) = web_sys::window()
        && let Some(document) = window.document()
        && let Some(log_el) = document.get_element_by_id("log")
    {
        let current = log_el.text_content().unwrap_or_default();
        let next = if current.is_empty() {
            line
        } else {
            format!("{}\n{}", current, line)
        };
        log_el.set_text_content(Some(&next));
    }

    Ok(())
}

fn set_module_status(message: &str) -> Result<(), JsValue> {
    set_textarea_value("module-output", message)
}

fn describe_js_error(error: &JsValue) -> String {
    error
        .as_string()
        .or_else(|| js_sys::JSON::stringify(error).ok().map(String::from))
        .unwrap_or_else(|| format!("{:?}", error))
}

async fn wait_for_connected(client: &WsClient) -> Result<(), JsValue> {
    for _ in 0..100 {
        if client.get_state() == "connected" {
            return Ok(());
        }
        sleep_ms(100).await?;
    }

    Err(JsValue::from_str("Timed out waiting for websocket connection"))
}

async fn sleep_ms(duration_ms: i32) -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
    let promise = Promise::new(&mut |resolve, reject| {
        let callback = Closure::once_into_js(move || {
            let _ = resolve.call0(&JsValue::NULL);
        });

        if let Err(error) =
            window.set_timeout_with_callback_and_timeout_and_arguments_0(callback.unchecked_ref(), duration_ms)
        {
            let _ = reject.call1(&JsValue::NULL, &error);
        }
    });
    JsFuture::from(promise).await.map(|_| ())
}

fn websocket_url() -> Result<String, JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
    let location = Reflect::get(window.as_ref(), &JsValue::from_str("location"))?;
    let protocol = Reflect::get(&location, &JsValue::from_str("protocol"))?
        .as_string()
        .ok_or_else(|| JsValue::from_str("window.location.protocol is unavailable"))?;
    let host = Reflect::get(&location, &JsValue::from_str("host"))?
        .as_string()
        .ok_or_else(|| JsValue::from_str("window.location.host is unavailable"))?;
    let ws_protocol = if protocol == "https:" { "wss:" } else { "ws:" };
    Ok(format!("{}//{}/ws", ws_protocol, host))
}
