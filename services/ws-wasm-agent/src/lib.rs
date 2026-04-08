use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;

use edge_toolkit::ws::{ConnectStatus, WsMessage};
use tracing::{error, info, warn};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Event, MediaStream, MediaStreamConstraints, MessageEvent, WebSocket};

const STORED_AGENT_ID_KEY: &str = "ws_wasm_agent.agent_id";

// Initialize logging for WASM
#[wasm_bindgen(start)]
pub fn init() {
    // Initialize tracing
    tracing_wasm::set_as_global_default();

    info!("WebSocket client initialized");
}

// Connection state
#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
}

#[wasm_bindgen]
pub struct MicrophoneAccess {
    stream: MediaStream,
}

#[wasm_bindgen]
pub struct VideoCapture {
    stream: MediaStream,
}

#[wasm_bindgen]
pub struct BluetoothAccess {
    device: JsValue,
}

#[wasm_bindgen]
pub struct GeolocationReading {
    latitude: f64,
    longitude: f64,
    accuracy_meters: f64,
}

#[wasm_bindgen]
pub struct GraphicsSupport {
    webgl_supported: bool,
    webgl2_supported: bool,
    webgpu_supported: bool,
    webnn_supported: bool,
}

#[wasm_bindgen]
pub struct WebGpuProbeResult {
    adapter_found: bool,
    device_created: bool,
}

#[wasm_bindgen]
pub struct GpuInfo {
    vendor: String,
    renderer: String,
    architecture: String,
    description: String,
    source: String,
}

#[wasm_bindgen]
pub struct SpeechRecognitionResult {
    transcript: String,
    confidence: f64,
}

#[wasm_bindgen]
pub struct SpeechRecognitionSession {
    recognition: JsValue,
    stop_requested: Rc<Cell<bool>>,
}

#[wasm_bindgen]
pub struct NfcScanResult {
    serial_number: String,
    record_summary: String,
}

#[wasm_bindgen]
impl MicrophoneAccess {
    #[wasm_bindgen(js_name = request)]
    pub async fn request() -> Result<MicrophoneAccess, JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
        let media_devices = get_media_devices(&window.navigator())?;

        let constraints = MediaStreamConstraints::new();
        constraints.set_audio(&JsValue::TRUE);
        constraints.set_video(&JsValue::FALSE);

        let promise = media_devices.get_user_media_with_constraints(&constraints)?;
        let stream = JsFuture::from(promise).await?;
        let stream: MediaStream = stream
            .dyn_into()
            .map_err(|_| JsValue::from_str("getUserMedia did not return a MediaStream"))?;

        info!(
            "Microphone access granted with {} audio track(s)",
            stream.get_audio_tracks().length()
        );

        Ok(MicrophoneAccess { stream })
    }

    #[wasm_bindgen(js_name = trackCount)]
    pub fn track_count(&self) -> u32 {
        self.stream.get_audio_tracks().length()
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
        info!("Microphone tracks stopped");
    }
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

#[wasm_bindgen]
impl BluetoothAccess {
    #[wasm_bindgen(js_name = request)]
    pub async fn request() -> Result<BluetoothAccess, JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
        let navigator = window.navigator();
        let bluetooth = js_sys::Reflect::get(&navigator, &JsValue::from_str("bluetooth"))?;
        if bluetooth.is_undefined() || bluetooth.is_null() {
            return Err(JsValue::from_str(
                "Web Bluetooth is not available in this browser context",
            ));
        }

        let options = js_sys::Object::new();
        js_sys::Reflect::set(&options, &JsValue::from_str("acceptAllDevices"), &JsValue::TRUE)?;

        let request_device = js_sys::Reflect::get(&bluetooth, &JsValue::from_str("requestDevice"))?
            .dyn_into::<js_sys::Function>()
            .map_err(|_| JsValue::from_str("navigator.bluetooth.requestDevice is not callable"))?;
        let promise = request_device
            .call1(&bluetooth, &options)?
            .dyn_into::<js_sys::Promise>()
            .map_err(|_| JsValue::from_str("requestDevice did not return a Promise"))?;
        let device = JsFuture::from(promise).await?;

        info!(
            "Bluetooth device selected: {:?}",
            js_sys::Reflect::get(&device, &JsValue::from_str("name"))
                .ok()
                .and_then(|value| value.as_string())
                .unwrap_or_else(|| "unknown".to_string())
        );

        Ok(BluetoothAccess { device })
    }

    pub fn id(&self) -> String {
        js_sys::Reflect::get(&self.device, &JsValue::from_str("id"))
            .ok()
            .and_then(|value| value.as_string())
            .unwrap_or_default()
    }

    pub fn name(&self) -> String {
        js_sys::Reflect::get(&self.device, &JsValue::from_str("name"))
            .ok()
            .and_then(|value| value.as_string())
            .unwrap_or_else(|| "unknown".to_string())
    }

    #[wasm_bindgen(js_name = gattConnected)]
    pub fn gatt_connected(&self) -> bool {
        js_sys::Reflect::get(&self.device, &JsValue::from_str("gatt"))
            .ok()
            .filter(|gatt| !gatt.is_null() && !gatt.is_undefined())
            .and_then(|gatt| js_sys::Reflect::get(&gatt, &JsValue::from_str("connected")).ok())
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
    }

    #[wasm_bindgen(js_name = connectGatt)]
    pub async fn connect_gatt(&self) -> Result<(), JsValue> {
        let gatt = js_sys::Reflect::get(&self.device, &JsValue::from_str("gatt"))?;
        if gatt.is_null() || gatt.is_undefined() {
            return Err(JsValue::from_str("Selected device has no GATT server"));
        }

        let connect = js_sys::Reflect::get(&gatt, &JsValue::from_str("connect"))?
            .dyn_into::<js_sys::Function>()
            .map_err(|_| JsValue::from_str("device.gatt.connect is not callable"))?;
        let promise = connect
            .call0(&gatt)?
            .dyn_into::<js_sys::Promise>()
            .map_err(|_| JsValue::from_str("device.gatt.connect did not return a Promise"))?;
        let _server = JsFuture::from(promise).await?;
        info!("Connected to Bluetooth GATT server for {}", self.name());
        Ok(())
    }
}

#[wasm_bindgen]
impl GeolocationReading {
    #[wasm_bindgen(js_name = request)]
    pub async fn request() -> Result<GeolocationReading, JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
        let navigator = window.navigator();
        let geolocation = js_sys::Reflect::get(&navigator, &JsValue::from_str("geolocation"))?;
        if geolocation.is_undefined() || geolocation.is_null() {
            return Err(JsValue::from_str(
                "navigator.geolocation is unavailable. Use https://... or http://localhost and allow access.",
            ));
        }

        let options = js_sys::Object::new();
        js_sys::Reflect::set(&options, &JsValue::from_str("enableHighAccuracy"), &JsValue::TRUE)?;
        js_sys::Reflect::set(&options, &JsValue::from_str("maximumAge"), &JsValue::from_f64(0.0))?;
        js_sys::Reflect::set(&options, &JsValue::from_str("timeout"), &JsValue::from_f64(10_000.0))?;

        let promise = js_sys::Promise::new(&mut |resolve, reject| {
            let reject_for_callback = reject.clone();
            let success = Closure::once(Box::new(move |position: JsValue| {
                let _ = resolve.call1(&JsValue::NULL, &position);
            }) as Box<dyn FnOnce(JsValue)>);

            let failure = Closure::once(Box::new(move |error: JsValue| {
                let _ = reject_for_callback.call1(&JsValue::NULL, &error);
            }) as Box<dyn FnOnce(JsValue)>);

            let get_current_position = js_sys::Reflect::get(&geolocation, &JsValue::from_str("getCurrentPosition"))
                .ok()
                .and_then(|value| value.dyn_into::<js_sys::Function>().ok());

            if let Some(get_current_position) = get_current_position {
                let _ = get_current_position.call3(
                    &geolocation,
                    success.as_ref().unchecked_ref(),
                    failure.as_ref().unchecked_ref(),
                    &options,
                );
            } else {
                let _ = reject.call1(
                    &JsValue::NULL,
                    &JsValue::from_str("navigator.geolocation.getCurrentPosition is not callable"),
                );
            }

            success.forget();
            failure.forget();
        });

        let position = JsFuture::from(promise).await?;
        let coords = js_sys::Reflect::get(&position, &JsValue::from_str("coords"))?;
        let latitude = js_sys::Reflect::get(&coords, &JsValue::from_str("latitude"))?
            .as_f64()
            .ok_or_else(|| JsValue::from_str("Geolocation latitude is missing"))?;
        let longitude = js_sys::Reflect::get(&coords, &JsValue::from_str("longitude"))?
            .as_f64()
            .ok_or_else(|| JsValue::from_str("Geolocation longitude is missing"))?;
        let accuracy_meters = js_sys::Reflect::get(&coords, &JsValue::from_str("accuracy"))?
            .as_f64()
            .ok_or_else(|| JsValue::from_str("Geolocation accuracy is missing"))?;

        info!(
            "Geolocation reading acquired: latitude={} longitude={} accuracy={}m",
            latitude, longitude, accuracy_meters
        );

        Ok(GeolocationReading {
            latitude,
            longitude,
            accuracy_meters,
        })
    }

    pub fn latitude(&self) -> f64 {
        self.latitude
    }

    pub fn longitude(&self) -> f64 {
        self.longitude
    }

    #[wasm_bindgen(js_name = accuracyMeters)]
    pub fn accuracy_meters(&self) -> f64 {
        self.accuracy_meters
    }
}

#[wasm_bindgen]
impl GraphicsSupport {
    #[wasm_bindgen(js_name = detect)]
    pub fn detect() -> Result<GraphicsSupport, JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
        let document = window
            .document()
            .ok_or_else(|| JsValue::from_str("No document available"))?;
        let canvas = document
            .create_element("canvas")?
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .map_err(|_| JsValue::from_str("Failed to create canvas element"))?;

        let webgl_supported = canvas.get_context("webgl")?.is_some();
        let webgl2_supported = canvas.get_context("webgl2")?.is_some();
        let webgpu_supported = js_sys::Reflect::get(&window.navigator(), &JsValue::from_str("gpu"))?.is_object();
        let webnn_supported = js_sys::Reflect::get(&window.navigator(), &JsValue::from_str("ml"))?.is_object();

        info!(
            "Graphics support detected: webgl={} webgl2={} webgpu={} webnn={}",
            webgl_supported, webgl2_supported, webgpu_supported, webnn_supported
        );

        Ok(GraphicsSupport {
            webgl_supported,
            webgl2_supported,
            webgpu_supported,
            webnn_supported,
        })
    }

    #[wasm_bindgen(js_name = webglSupported)]
    pub fn webgl_supported(&self) -> bool {
        self.webgl_supported
    }

    #[wasm_bindgen(js_name = webgl2Supported)]
    pub fn webgl2_supported(&self) -> bool {
        self.webgl2_supported
    }

    #[wasm_bindgen(js_name = webgpuSupported)]
    pub fn webgpu_supported(&self) -> bool {
        self.webgpu_supported
    }

    #[wasm_bindgen(js_name = webnnSupported)]
    pub fn webnn_supported(&self) -> bool {
        self.webnn_supported
    }
}

#[wasm_bindgen]
impl WebGpuProbeResult {
    #[wasm_bindgen(js_name = test)]
    pub async fn test() -> Result<WebGpuProbeResult, JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
        let navigator = window.navigator();
        let gpu = js_sys::Reflect::get(&navigator, &JsValue::from_str("gpu"))?;

        if gpu.is_null() || gpu.is_undefined() {
            return Ok(WebGpuProbeResult {
                adapter_found: false,
                device_created: false,
            });
        }

        let request_adapter = js_sys::Reflect::get(&gpu, &JsValue::from_str("requestAdapter"))?
            .dyn_into::<js_sys::Function>()
            .map_err(|_| JsValue::from_str("navigator.gpu.requestAdapter is not callable"))?;

        let adapter_promise = request_adapter
            .call0(&gpu)?
            .dyn_into::<js_sys::Promise>()
            .map_err(|_| JsValue::from_str("requestAdapter did not return a Promise"))?;
        let adapter = JsFuture::from(adapter_promise).await?;

        if adapter.is_null() || adapter.is_undefined() {
            info!("WebGPU probe: no adapter available");
            return Ok(WebGpuProbeResult {
                adapter_found: false,
                device_created: false,
            });
        }

        let request_device = js_sys::Reflect::get(&adapter, &JsValue::from_str("requestDevice"))?
            .dyn_into::<js_sys::Function>()
            .map_err(|_| JsValue::from_str("adapter.requestDevice is not callable"))?;

        let device_promise = request_device
            .call0(&adapter)?
            .dyn_into::<js_sys::Promise>()
            .map_err(|_| JsValue::from_str("requestDevice did not return a Promise"))?;
        let device = JsFuture::from(device_promise).await?;

        let device_created = !device.is_null() && !device.is_undefined();
        info!(
            "WebGPU probe completed: adapter_found=true device_created={}",
            device_created
        );

        Ok(WebGpuProbeResult {
            adapter_found: true,
            device_created,
        })
    }

    #[wasm_bindgen(js_name = adapterFound)]
    pub fn adapter_found(&self) -> bool {
        self.adapter_found
    }

    #[wasm_bindgen(js_name = deviceCreated)]
    pub fn device_created(&self) -> bool {
        self.device_created
    }
}

#[wasm_bindgen]
impl GpuInfo {
    #[wasm_bindgen(js_name = detect)]
    pub async fn detect() -> Result<GpuInfo, JsValue> {
        if let Some(info) = detect_webgpu_info().await? {
            return Ok(info);
        }

        if let Some(info) = detect_webgl_info()? {
            return Ok(info);
        }

        Ok(GpuInfo {
            vendor: "unknown".to_string(),
            renderer: "unknown".to_string(),
            architecture: "unknown".to_string(),
            description: "No GPU details exposed by this browser".to_string(),
            source: "none".to_string(),
        })
    }

    pub fn vendor(&self) -> String {
        self.vendor.clone()
    }

    pub fn renderer(&self) -> String {
        self.renderer.clone()
    }

    pub fn architecture(&self) -> String {
        self.architecture.clone()
    }

    pub fn description(&self) -> String {
        self.description.clone()
    }

    pub fn source(&self) -> String {
        self.source.clone()
    }
}

#[wasm_bindgen]
impl SpeechRecognitionResult {
    #[wasm_bindgen(js_name = recognizeOnce)]
    pub async fn recognize_once() -> Result<SpeechRecognitionResult, JsValue> {
        let session = SpeechRecognitionSession::new()?;
        session.start().await
    }

    pub fn transcript(&self) -> String {
        self.transcript.clone()
    }

    pub fn confidence(&self) -> f64 {
        self.confidence
    }
}

#[wasm_bindgen]
impl SpeechRecognitionSession {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Result<SpeechRecognitionSession, JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
        let speech_recognition_ctor = js_sys::Reflect::get(&window, &JsValue::from_str("SpeechRecognition"))
            .ok()
            .filter(|value| !value.is_undefined() && !value.is_null())
            .or_else(|| {
                js_sys::Reflect::get(&window, &JsValue::from_str("webkitSpeechRecognition"))
                    .ok()
                    .filter(|value| !value.is_undefined() && !value.is_null())
            })
            .ok_or_else(|| JsValue::from_str("Web Speech API recognition is not available in this browser context"))?;
        let constructor = speech_recognition_ctor
            .dyn_into::<js_sys::Function>()
            .map_err(|_| JsValue::from_str("SpeechRecognition constructor is not callable"))?;
        let recognition = js_sys::Reflect::construct(&constructor, &js_sys::Array::new())?;

        js_sys::Reflect::set(&recognition, &JsValue::from_str("lang"), &JsValue::from_str("en-US"))?;
        js_sys::Reflect::set(&recognition, &JsValue::from_str("interimResults"), &JsValue::TRUE)?;
        js_sys::Reflect::set(
            &recognition,
            &JsValue::from_str("maxAlternatives"),
            &JsValue::from_f64(1.0),
        )?;

        Ok(SpeechRecognitionSession {
            recognition,
            stop_requested: Rc::new(Cell::new(false)),
        })
    }

    pub async fn start(&self) -> Result<SpeechRecognitionResult, JsValue> {
        self.stop_requested.set(false);
        let recognition = self.recognition.clone();
        let stop_requested = self.stop_requested.clone();
        let promise = js_sys::Promise::new(&mut |resolve, reject| {
            let settled = Rc::new(Cell::new(false));
            let resolve_for_result = resolve.clone();
            let resolve_for_end = resolve.clone();
            let reject_for_error = reject.clone();
            let reject_for_end = reject.clone();
            let settled_for_result = settled.clone();
            let settled_for_error = settled.clone();
            let settled_for_end = settled.clone();
            let transcript_state: Rc<RefCell<Option<(String, f64)>>> = Rc::new(RefCell::new(None));
            let transcript_state_for_result = transcript_state.clone();
            let transcript_state_for_end = transcript_state.clone();
            let stop_requested_for_end = stop_requested.clone();

            let on_result = Closure::wrap(Box::new(move |event: JsValue| {
                if let Some((transcript, confidence, has_final)) = extract_speech_event_transcript(&event) {
                    *transcript_state_for_result.borrow_mut() = Some((transcript.clone(), confidence));

                    if has_final && !settled_for_result.replace(true) {
                        let payload = js_sys::Object::new();
                        let _ = js_sys::Reflect::set(
                            &payload,
                            &JsValue::from_str("transcript"),
                            &JsValue::from_str(&transcript),
                        );
                        let _ = js_sys::Reflect::set(
                            &payload,
                            &JsValue::from_str("confidence"),
                            &JsValue::from_f64(confidence),
                        );
                        let _ = resolve_for_result.call1(&JsValue::NULL, &payload);
                    }
                }
            }) as Box<dyn FnMut(JsValue)>);

            let on_error = Closure::wrap(Box::new(move |event: JsValue| {
                if settled_for_error.replace(true) {
                    return;
                }
                let message = js_sys::Reflect::get(&event, &JsValue::from_str("error"))
                    .ok()
                    .and_then(|value| value.as_string())
                    .unwrap_or_else(|| "speech recognition failed".to_string());
                let _ = reject_for_error.call1(&JsValue::NULL, &JsValue::from_str(&message));
            }) as Box<dyn FnMut(JsValue)>);

            let on_end = Closure::wrap(Box::new(move || {
                if settled_for_end.replace(true) {
                    return;
                }
                if let Some((transcript, confidence)) = transcript_state_for_end.borrow().clone() {
                    let payload = js_sys::Object::new();
                    let _ = js_sys::Reflect::set(
                        &payload,
                        &JsValue::from_str("transcript"),
                        &JsValue::from_str(&transcript),
                    );
                    let _ = js_sys::Reflect::set(
                        &payload,
                        &JsValue::from_str("confidence"),
                        &JsValue::from_f64(confidence),
                    );
                    let _ = resolve_for_end.call1(&JsValue::NULL, &payload);
                } else if stop_requested_for_end.get() {
                    let _ = reject_for_end.call1(
                        &JsValue::NULL,
                        &JsValue::from_str("speech recognition stopped before any transcript was captured"),
                    );
                } else {
                    let _ = reject_for_end.call1(
                        &JsValue::NULL,
                        &JsValue::from_str("speech recognition ended without a transcript"),
                    );
                }
            }) as Box<dyn FnMut()>);

            let _ = js_sys::Reflect::set(
                &recognition,
                &JsValue::from_str("onresult"),
                on_result.as_ref().unchecked_ref(),
            );
            let _ = js_sys::Reflect::set(
                &recognition,
                &JsValue::from_str("onerror"),
                on_error.as_ref().unchecked_ref(),
            );
            let _ = js_sys::Reflect::set(
                &recognition,
                &JsValue::from_str("onend"),
                on_end.as_ref().unchecked_ref(),
            );

            if let Some(start) = js_sys::Reflect::get(&recognition, &JsValue::from_str("start"))
                .ok()
                .and_then(|value| value.dyn_into::<js_sys::Function>().ok())
            {
                let _ = start.call0(&recognition);
            } else {
                let _ = reject.call1(
                    &JsValue::NULL,
                    &JsValue::from_str("SpeechRecognition.start is not callable"),
                );
            }

            on_result.forget();
            on_error.forget();
            on_end.forget();
        });

        let result = JsFuture::from(promise).await?;
        let transcript = js_sys::Reflect::get(&result, &JsValue::from_str("transcript"))?
            .as_string()
            .ok_or_else(|| JsValue::from_str("Speech recognition transcript missing"))?;
        let confidence = js_sys::Reflect::get(&result, &JsValue::from_str("confidence"))?
            .as_f64()
            .unwrap_or(0.0);

        info!("Speech recognition captured transcript with confidence={}", confidence);

        Ok(SpeechRecognitionResult { transcript, confidence })
    }

    pub fn stop(&self) -> Result<(), JsValue> {
        self.stop_requested.set(true);
        let stop = js_sys::Reflect::get(&self.recognition, &JsValue::from_str("stop"))?
            .dyn_into::<js_sys::Function>()
            .map_err(|_| JsValue::from_str("SpeechRecognition.stop is not callable"))?;
        stop.call0(&self.recognition)?;
        Ok(())
    }
}

#[wasm_bindgen]
impl NfcScanResult {
    #[wasm_bindgen(js_name = scanOnce)]
    pub async fn scan_once() -> Result<NfcScanResult, JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
        let ndef_ctor = js_sys::Reflect::get(&window, &JsValue::from_str("NDEFReader"))
            .ok()
            .filter(|value| !value.is_undefined() && !value.is_null())
            .ok_or_else(|| JsValue::from_str("Web NFC is not available in this browser context"))?;

        let constructor = ndef_ctor
            .dyn_into::<js_sys::Function>()
            .map_err(|_| JsValue::from_str("NDEFReader constructor is not callable"))?;
        let reader = js_sys::Reflect::construct(&constructor, &js_sys::Array::new())?;

        let scan = js_sys::Reflect::get(&reader, &JsValue::from_str("scan"))?
            .dyn_into::<js_sys::Function>()
            .map_err(|_| JsValue::from_str("NDEFReader.scan is not callable"))?;
        let scan_promise = scan
            .call0(&reader)?
            .dyn_into::<js_sys::Promise>()
            .map_err(|_| JsValue::from_str("NDEFReader.scan did not return a Promise"))?;
        let _ = JsFuture::from(scan_promise).await?;

        let promise = js_sys::Promise::new(&mut |resolve, reject| {
            let reject_for_timeout = reject.clone();
            let timeout_closure = Closure::once(Box::new(move || {
                let _ = reject_for_timeout.call1(
                    &JsValue::NULL,
                    &JsValue::from_str("NFC scan timed out after 20 seconds"),
                );
            }) as Box<dyn FnOnce()>);

            if let Some(window) = web_sys::window() {
                let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                    timeout_closure.as_ref().unchecked_ref(),
                    20_000,
                );
            }

            let reject_for_error = reject.clone();

            let on_reading = Closure::once(Box::new(move |event: JsValue| {
                let serial_number = js_sys::Reflect::get(&event, &JsValue::from_str("serialNumber"))
                    .ok()
                    .and_then(|value| value.as_string())
                    .unwrap_or_else(|| "unknown".to_string());
                let record_summary = summarize_ndef_records(&event);

                let payload = js_sys::Object::new();
                let _ = js_sys::Reflect::set(
                    &payload,
                    &JsValue::from_str("serialNumber"),
                    &JsValue::from_str(&serial_number),
                );
                let _ = js_sys::Reflect::set(
                    &payload,
                    &JsValue::from_str("recordSummary"),
                    &JsValue::from_str(&record_summary),
                );
                let _ = resolve.call1(&JsValue::NULL, &payload);
            }) as Box<dyn FnOnce(JsValue)>);

            let on_reading_error = Closure::once(Box::new(move |event: JsValue| {
                let message = js_sys::Reflect::get(&event, &JsValue::from_str("message"))
                    .ok()
                    .and_then(|value| value.as_string())
                    .unwrap_or_else(|| "NFC reading failed".to_string());
                let _ = reject_for_error.call1(&JsValue::NULL, &JsValue::from_str(&message));
            }) as Box<dyn FnOnce(JsValue)>);

            let _ = js_sys::Reflect::set(
                &reader,
                &JsValue::from_str("onreading"),
                on_reading.as_ref().unchecked_ref(),
            );
            let _ = js_sys::Reflect::set(
                &reader,
                &JsValue::from_str("onreadingerror"),
                on_reading_error.as_ref().unchecked_ref(),
            );

            on_reading.forget();
            on_reading_error.forget();
            timeout_closure.forget();
        });

        let result = JsFuture::from(promise).await?;
        let serial_number = js_sys::Reflect::get(&result, &JsValue::from_str("serialNumber"))?
            .as_string()
            .unwrap_or_else(|| "unknown".to_string());
        let record_summary = js_sys::Reflect::get(&result, &JsValue::from_str("recordSummary"))?
            .as_string()
            .unwrap_or_else(|| "no records".to_string());

        info!(
            "NFC scan captured: serial_number={} summary={}",
            serial_number, record_summary
        );

        Ok(NfcScanResult {
            serial_number,
            record_summary,
        })
    }

    #[wasm_bindgen(js_name = serialNumber)]
    pub fn serial_number(&self) -> String {
        self.serial_number.clone()
    }

    #[wasm_bindgen(js_name = recordSummary)]
    pub fn record_summary(&self) -> String {
        self.record_summary.clone()
    }
}

fn get_media_devices(navigator: &web_sys::Navigator) -> Result<web_sys::MediaDevices, JsValue> {
    let media_devices = js_sys::Reflect::get(navigator, &JsValue::from_str("mediaDevices"))?;

    if media_devices.is_undefined() || media_devices.is_null() {
        return Err(JsValue::from_str(
            "navigator.mediaDevices is unavailable. Use https://... or http://localhost and allow access.",
        ));
    }

    media_devices
        .dyn_into::<web_sys::MediaDevices>()
        .map_err(|_| JsValue::from_str("navigator.mediaDevices is not accessible in this browser"))
}

async fn detect_webgpu_info() -> Result<Option<GpuInfo>, JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
    let navigator = window.navigator();
    let gpu = js_sys::Reflect::get(&navigator, &JsValue::from_str("gpu"))?;

    if gpu.is_null() || gpu.is_undefined() {
        return Ok(None);
    }

    let request_adapter = match js_sys::Reflect::get(&gpu, &JsValue::from_str("requestAdapter"))
        .ok()
        .and_then(|value| value.dyn_into::<js_sys::Function>().ok())
    {
        Some(request_adapter) => request_adapter,
        None => return Ok(None),
    };

    let adapter_promise = request_adapter
        .call0(&gpu)?
        .dyn_into::<js_sys::Promise>()
        .map_err(|_| JsValue::from_str("requestAdapter did not return a Promise"))?;
    let adapter = JsFuture::from(adapter_promise).await?;

    if adapter.is_null() || adapter.is_undefined() {
        return Ok(None);
    }

    let info_object = if let Some(request_adapter_info) =
        js_sys::Reflect::get(&adapter, &JsValue::from_str("requestAdapterInfo"))
            .ok()
            .and_then(|value| value.dyn_into::<js_sys::Function>().ok())
    {
        let info_promise = request_adapter_info
            .call0(&adapter)?
            .dyn_into::<js_sys::Promise>()
            .map_err(|_| JsValue::from_str("requestAdapterInfo did not return a Promise"))?;
        JsFuture::from(info_promise).await?
    } else {
        js_sys::Reflect::get(&adapter, &JsValue::from_str("info"))?
    };

    if info_object.is_null() || info_object.is_undefined() {
        return Ok(None);
    }

    let vendor = js_string_field(&info_object, "vendor");
    let architecture = js_string_field(&info_object, "architecture");
    let description = js_string_field(&info_object, "description");
    let device = js_string_field(&info_object, "device");
    let renderer = if device.is_empty() { description.clone() } else { device };

    Ok(Some(GpuInfo {
        vendor: string_or_unknown(vendor),
        renderer: string_or_unknown(renderer),
        architecture: string_or_unknown(architecture),
        description: string_or_unknown(description),
        source: "webgpu".to_string(),
    }))
}

fn detect_webgl_info() -> Result<Option<GpuInfo>, JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
    let document = window
        .document()
        .ok_or_else(|| JsValue::from_str("No document available"))?;
    let canvas = document
        .create_element("canvas")?
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .map_err(|_| JsValue::from_str("Failed to create canvas element"))?;

    let context = canvas
        .get_context("webgl")?
        .or_else(|| canvas.get_context("webgl2").ok().flatten());

    let Some(context) = context else {
        return Ok(None);
    };

    let get_extension = match js_sys::Reflect::get(&context, &JsValue::from_str("getExtension"))
        .ok()
        .and_then(|value| value.dyn_into::<js_sys::Function>().ok())
    {
        Some(get_extension) => get_extension,
        None => return Ok(None),
    };

    let extension = get_extension.call1(&context, &JsValue::from_str("WEBGL_debug_renderer_info"))?;
    if extension.is_null() || extension.is_undefined() {
        return Ok(None);
    }

    let get_parameter = match js_sys::Reflect::get(&context, &JsValue::from_str("getParameter"))
        .ok()
        .and_then(|value| value.dyn_into::<js_sys::Function>().ok())
    {
        Some(get_parameter) => get_parameter,
        None => return Ok(None),
    };

    let vendor_enum = js_sys::Reflect::get(&extension, &JsValue::from_str("UNMASKED_VENDOR_WEBGL"))?;
    let renderer_enum = js_sys::Reflect::get(&extension, &JsValue::from_str("UNMASKED_RENDERER_WEBGL"))?;

    let vendor = get_parameter
        .call1(&context, &vendor_enum)
        .ok()
        .and_then(|value| value.as_string())
        .unwrap_or_else(|| "unknown".to_string());
    let renderer = get_parameter
        .call1(&context, &renderer_enum)
        .ok()
        .and_then(|value| value.as_string())
        .unwrap_or_else(|| "unknown".to_string());

    Ok(Some(GpuInfo {
        vendor,
        renderer: renderer.clone(),
        architecture: "unknown".to_string(),
        description: renderer,
        source: "webgl_debug_renderer_info".to_string(),
    }))
}

fn js_string_field(value: &JsValue, field: &str) -> String {
    js_sys::Reflect::get(value, &JsValue::from_str(field))
        .ok()
        .and_then(|field_value| field_value.as_string())
        .unwrap_or_default()
}

fn string_or_unknown(value: String) -> String {
    if value.is_empty() {
        "unknown".to_string()
    } else {
        value
    }
}

fn extract_speech_event_transcript(event: &JsValue) -> Option<(String, f64, bool)> {
    let results = js_sys::Reflect::get(event, &JsValue::from_str("results")).ok()?;
    let length = js_sys::Reflect::get(&results, &JsValue::from_str("length"))
        .ok()?
        .as_f64()? as u32;

    let mut transcript_parts = Vec::new();
    let mut confidence = 0.0;
    let mut confidence_count = 0_u32;
    let mut has_final = false;

    for index in 0..length {
        let result = match js_sys::Reflect::get(&results, &JsValue::from_f64(index as f64)) {
            Ok(result) => result,
            Err(_) => continue,
        };

        let alternative = match js_sys::Reflect::get(&result, &JsValue::from_f64(0.0)) {
            Ok(alternative) => alternative,
            Err(_) => continue,
        };

        if let Some(part) = js_sys::Reflect::get(&alternative, &JsValue::from_str("transcript"))
            .ok()
            .and_then(|value| value.as_string())
        {
            let trimmed = part.trim();
            if !trimmed.is_empty() {
                transcript_parts.push(trimmed.to_string());
            }
        }

        if let Some(value) = js_sys::Reflect::get(&alternative, &JsValue::from_str("confidence"))
            .ok()
            .and_then(|value| value.as_f64())
        {
            confidence += value;
            confidence_count += 1;
        }

        if js_sys::Reflect::get(&result, &JsValue::from_str("isFinal"))
            .ok()
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            has_final = true;
        }
    }

    if transcript_parts.is_empty() {
        return None;
    }

    let transcript = transcript_parts.join(" ");
    let average_confidence = if confidence_count == 0 {
        0.0
    } else {
        confidence / confidence_count as f64
    };

    Some((transcript, average_confidence, has_final))
}

fn summarize_ndef_records(event: &JsValue) -> String {
    let message = match js_sys::Reflect::get(event, &JsValue::from_str("message")) {
        Ok(message) => message,
        Err(_) => return "no message".to_string(),
    };
    let records = match js_sys::Reflect::get(&message, &JsValue::from_str("records")) {
        Ok(records) => records,
        Err(_) => return "no records".to_string(),
    };
    let length = match js_sys::Reflect::get(&records, &JsValue::from_str("length"))
        .ok()
        .and_then(|value| value.as_f64())
    {
        Some(length) => length as u32,
        None => return "no records".to_string(),
    };

    let mut summary = Vec::new();
    for index in 0..length {
        let record = match js_sys::Reflect::get(&records, &JsValue::from_f64(index as f64)) {
            Ok(record) => record,
            Err(_) => continue,
        };
        let record_type = js_sys::Reflect::get(&record, &JsValue::from_str("recordType"))
            .ok()
            .and_then(|value| value.as_string())
            .unwrap_or_else(|| "unknown".to_string());
        let media_type = js_sys::Reflect::get(&record, &JsValue::from_str("mediaType"))
            .ok()
            .and_then(|value| value.as_string())
            .unwrap_or_default();
        let id = js_sys::Reflect::get(&record, &JsValue::from_str("id"))
            .ok()
            .and_then(|value| value.as_string())
            .unwrap_or_default();

        let mut parts = vec![format!("type={record_type}")];
        if !media_type.is_empty() {
            parts.push(format!("media={media_type}"));
        }
        if !id.is_empty() {
            parts.push(format!("id={id}"));
        }
        summary.push(parts.join(","));
    }

    if summary.is_empty() {
        "no records".to_string()
    } else {
        summary.join(" | ")
    }
}

// WebSocket client configuration
#[wasm_bindgen]
pub struct WsClientConfig {
    server_url: String,
    alive_interval_ms: u32,
    max_reconnect_attempts: u32,
    initial_reconnect_delay_ms: u32,
}

#[wasm_bindgen]
impl WsClientConfig {
    #[wasm_bindgen(constructor)]
    pub fn new(server_url: String) -> WsClientConfig {
        WsClientConfig {
            server_url,
            alive_interval_ms: 5000,
            max_reconnect_attempts: 10,
            initial_reconnect_delay_ms: 1000,
        }
    }

    #[wasm_bindgen(setter)]
    pub fn set_alive_interval(&mut self, interval_ms: u32) {
        self.alive_interval_ms = interval_ms;
    }

    #[wasm_bindgen(setter)]
    pub fn set_max_reconnect_attempts(&mut self, attempts: u32) {
        self.max_reconnect_attempts = attempts;
    }

    #[wasm_bindgen(setter)]
    pub fn set_initial_reconnect_delay(&mut self, delay_ms: u32) {
        self.initial_reconnect_delay_ms = delay_ms;
    }
}

// Inner shared state
struct SharedState {
    socket: Option<WebSocket>,
    state: ConnectionState,
    _alive_interval_id: Option<JsValue>,
    reconnect_attempts: u32,
    reconnect_delay_ms: u32,
    on_message_callback: Option<JsValue>,
    on_state_change_callback: Option<JsValue>,
}

// Main WebSocket client
#[wasm_bindgen]
pub struct WsClient {
    config: WsClientConfig,
    agent_id: Rc<RefCell<Option<String>>>,
    shared: Rc<RefCell<SharedState>>,
}

#[wasm_bindgen]
impl WsClient {
    #[wasm_bindgen(constructor)]
    pub fn new(config: WsClientConfig) -> WsClient {
        let agent_id = load_stored_agent_id();
        info!("Creating new WebSocket client with retained agent ID: {:?}", agent_id);

        let shared = Rc::new(RefCell::new(SharedState {
            socket: None,
            state: ConnectionState::Disconnected,
            _alive_interval_id: None,
            reconnect_attempts: 0,
            reconnect_delay_ms: 1000,
            on_message_callback: None,
            on_state_change_callback: None,
        }));

        WsClient {
            config,
            agent_id: Rc::new(RefCell::new(agent_id)),
            shared,
        }
    }

    /// Connect to the WebSocket server
    #[wasm_bindgen]
    pub fn connect(&mut self) -> Result<(), JsValue> {
        info!("Connecting to WebSocket server: {}", self.config.server_url);

        let _window = web_sys::window().ok_or("No window available")?;
        let socket = WebSocket::new(&self.config.server_url)
            .map_err(|e| JsValue::from_str(&format!("Failed to create WebSocket: {:?}", e)))?;

        // Set binary type to arraybuffer
        socket.set_binary_type(web_sys::BinaryType::Arraybuffer);

        // Store the socket
        {
            let mut s = self.shared.borrow_mut();
            s.socket = Some(socket.clone());
            s.state = ConnectionState::Connecting;
        }
        self.notify_state_change();

        // Set up event handlers
        let on_open = Closure::wrap(Box::new({
            let shared = self.shared.clone();
            let initial_delay = self.config.initial_reconnect_delay_ms;
            let cli_ptr = self.clone();
            move |_event: Event| {
                info!("WebSocket connected");
                {
                    let mut s = shared.borrow_mut();
                    s.state = ConnectionState::Connected;
                    s.reconnect_attempts = 0;
                    s.reconnect_delay_ms = initial_delay;
                }
                cli_ptr.notify_state_change();
                if let Err(error) = cli_ptr.send_connect_message() {
                    error!("Failed to send connect message: {:?}", error);
                }
                cli_ptr.start_alive_interval();
            }
        }) as Box<dyn FnMut(Event)>);

        let on_message = Closure::wrap(Box::new({
            let shared = self.shared.clone();
            let retained_agent_id = self.agent_id.clone();
            move |event: MessageEvent| {
                info!("WebSocket message received");
                if let Some(data) = event.data().as_string() {
                    info!("Received: {}", data);
                    // Try to parse and handle the message
                    if let Ok(msg) = serde_json::from_str::<WsMessage>(&data) {
                        match msg {
                            WsMessage::ConnectAck { agent_id, status } => {
                                info!(
                                    "Server connect acknowledgement: agent_id={} status={:?}",
                                    agent_id, status
                                );
                                *retained_agent_id.borrow_mut() = Some(agent_id.clone());
                                if let Err(error) = store_agent_id(&agent_id) {
                                    warn!("Failed to persist agent ID: {:?}", error);
                                }
                                match status {
                                    ConnectStatus::Assigned => {
                                        info!("Server assigned a new agent_id");
                                    }
                                    ConnectStatus::Reconnected => {
                                        info!("Server accepted retained agent_id");
                                    }
                                }
                            }
                            WsMessage::Response { message } => {
                                info!("Server response: {}", message);
                            }
                            WsMessage::Alive { .. } => {
                                warn!("Unexpected alive message from server");
                            }
                            WsMessage::ClientEvent { .. } => {
                                warn!("Unexpected client_event message from server");
                            }
                            WsMessage::Connect { .. } => {
                                warn!("Unexpected connect message from server");
                            }
                        }
                    }
                    // Notify callback if set
                    let s = shared.borrow();
                    if let Some(ref callback) = s.on_message_callback {
                        if let Some(function) = callback.dyn_ref::<js_sys::Function>() {
                            let _ = function.call1(&JsValue::NULL, &JsValue::from_str(&data));
                        }
                    }
                }
            }
        }) as Box<dyn FnMut(MessageEvent)>);

        let on_error = Closure::wrap(Box::new({
            let mut cli_ptr = self.clone();
            move |_event: Event| {
                error!("WebSocket error occurred");
                cli_ptr.handle_disconnect();
            }
        }) as Box<dyn FnMut(Event)>);

        let on_close = Closure::wrap(Box::new({
            let mut cli_ptr = self.clone();
            move |_event: Event| {
                info!("WebSocket closed");
                cli_ptr.handle_disconnect();
            }
        }) as Box<dyn FnMut(Event)>);

        // Add event listeners
        socket.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        socket.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));

        // Forget the closures to keep them alive
        on_open.forget();
        on_message.forget();
        on_error.forget();
        on_close.forget();

        Ok(())
    }

    /// Disconnect from the WebSocket server
    #[wasm_bindgen]
    pub fn disconnect(&mut self) {
        info!("Disconnecting WebSocket client");
        self.stop_alive_interval();
        {
            let mut s = self.shared.borrow_mut();
            if let Some(ref socket) = s.socket {
                let _ = socket.close();
            }
            s.socket = None;
            s.state = ConnectionState::Disconnected;
        }
        self.notify_state_change();
    }

    /// Send an alive message to the server
    #[wasm_bindgen]
    pub fn send_alive(&self) -> Result<(), JsValue> {
        let s = self.shared.borrow();
        if s.state != ConnectionState::Connected {
            return Err(JsValue::from_str("Not connected"));
        }

        let timestamp = chrono::Utc::now().to_rfc3339();
        let msg = WsMessage::Alive { timestamp };

        let json = serde_json::to_string(&msg)
            .map_err(|e| JsValue::from_str(&format!("Failed to serialize message: {}", e)))?;

        if let Some(ref socket) = s.socket {
            socket
                .send_with_str(&json)
                .map_err(|e| JsValue::from_str(&format!("Failed to send message: {:?}", e)))?;
            info!("Alive message sent: {}", json);
        }

        Ok(())
    }

    /// Send a custom message to the server
    #[wasm_bindgen]
    pub fn send(&self, message: &str) -> Result<(), JsValue> {
        let s = self.shared.borrow();
        if s.state != ConnectionState::Connected {
            return Err(JsValue::from_str("Not connected"));
        }

        if let Some(ref socket) = s.socket {
            socket
                .send_with_str(message)
                .map_err(|e| JsValue::from_str(&format!("Failed to send message: {:?}", e)))?;
            info!("Message sent: {}", message);
        }

        Ok(())
    }

    /// Get the current connection state
    #[wasm_bindgen]
    pub fn get_state(&self) -> String {
        match self.shared.borrow().state {
            ConnectionState::Disconnected => "disconnected".to_string(),
            ConnectionState::Connecting => "connecting".to_string(),
            ConnectionState::Connected => "connected".to_string(),
            ConnectionState::Reconnecting => "reconnecting".to_string(),
        }
    }

    /// Get the client ID
    #[wasm_bindgen]
    pub fn get_client_id(&self) -> String {
        self.agent_id.borrow().clone().unwrap_or_default()
    }

    /// Set callback for message events
    #[wasm_bindgen]
    pub fn set_on_message(&mut self, callback: JsValue) {
        self.shared.borrow_mut().on_message_callback = Some(callback);
    }

    /// Set callback for state change events
    #[wasm_bindgen]
    pub fn set_on_state_change(&mut self, callback: JsValue) {
        self.shared.borrow_mut().on_state_change_callback = Some(callback);
    }

    // Internal methods

    fn start_alive_interval(&self) {
        info!("Starting alive interval");
        // Note: In a real implementation, we'd use set_interval with Closure
    }

    fn stop_alive_interval(&self) {
        info!("Stopping alive interval");
    }

    fn handle_disconnect(&mut self) {
        self.stop_alive_interval();
        {
            let mut s = self.shared.borrow_mut();
            s.state = ConnectionState::Disconnected;
        }
        self.notify_state_change();

        // Attempt reconnection with exponential backoff
        let mut do_reconnect = false;
        let mut next_delay = 0;
        let mut curr_attempt = 0;
        {
            let mut s = self.shared.borrow_mut();
            if s.reconnect_attempts < self.config.max_reconnect_attempts {
                s.state = ConnectionState::Reconnecting;
                next_delay = s.reconnect_delay_ms;
                s.reconnect_delay_ms = (s.reconnect_delay_ms * 2).min(30000);
                s.reconnect_attempts += 1;
                curr_attempt = s.reconnect_attempts;
                do_reconnect = true;
            }
        }
        if do_reconnect {
            self.notify_state_change();
            info!("Attempting reconnection {} in {}ms", curr_attempt, next_delay);
        } else {
            error!("Max reconnection attempts reached");
        }
    }

    fn notify_state_change(&self) {
        let state = self.get_state();
        let s = self.shared.borrow();
        if let Some(ref callback) = s.on_state_change_callback {
            if let Some(function) = callback.dyn_ref::<js_sys::Function>() {
                let _ = function.call1(&JsValue::NULL, &JsValue::from_str(&state));
            }
        }
    }

    fn send_connect_message(&self) -> Result<(), JsValue> {
        let s = self.shared.borrow();
        let msg = WsMessage::Connect {
            agent_id: self.agent_id.borrow().clone(),
        };

        let json = serde_json::to_string(&msg)
            .map_err(|e| JsValue::from_str(&format!("Failed to serialize connect message: {}", e)))?;

        if let Some(ref socket) = s.socket {
            socket
                .send_with_str(&json)
                .map_err(|e| JsValue::from_str(&format!("Failed to send connect message: {:?}", e)))?;
            info!("Connect message sent: {}", json);
        }

        Ok(())
    }
}

// Implement Clone for WsClient (required for closures)
impl Clone for WsClient {
    fn clone(&self) -> WsClient {
        WsClient {
            config: WsClientConfig {
                server_url: self.config.server_url.clone(),
                alive_interval_ms: self.config.alive_interval_ms,
                max_reconnect_attempts: self.config.max_reconnect_attempts,
                initial_reconnect_delay_ms: self.config.initial_reconnect_delay_ms,
            },
            agent_id: self.agent_id.clone(),
            shared: self.shared.clone(),
        }
    }
}

// Helper function to create a client and connect
#[wasm_bindgen]
pub fn create_and_connect(server_url: String) -> Result<WsClient, JsValue> {
    let config = WsClientConfig::new(server_url);
    let mut client = WsClient::new(config);
    client.connect()?;
    Ok(client)
}

fn load_stored_agent_id() -> Option<String> {
    let window = web_sys::window()?;
    let storage = window.local_storage().ok()??;
    storage.get_item(STORED_AGENT_ID_KEY).ok()?
}

fn store_agent_id(agent_id: &str) -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
    let storage = window
        .local_storage()?
        .ok_or_else(|| JsValue::from_str("No localStorage available"))?;
    storage.set_item(STORED_AGENT_ID_KEY, agent_id)
}
