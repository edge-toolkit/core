use std::cell::Cell;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use edge_toolkit::ws::{ConnectStatus, WsMessage};
use tracing::{error, info, warn};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Event, MediaStream, MediaStreamConstraints, MessageEvent, WebSocket};

const STORED_AGENT_ID_KEY: &str = "ws_wasm_agent.agent_id";
const STORED_LAST_OFFLINE_AT_KEY: &str = "ws_wasm_agent.last_offline_at";
const MAX_OFFLINE_QUEUE_LEN: usize = 1000;
/// Default cadence for client-side app-level `Alive` messages sent to the websocket server.
/// This should remain comfortably lower than the server's idle connection timeout.
const DEFAULT_ALIVE_INTERVAL_MS: u32 = 5_000;
const SENSOR_PERMISSION_GRANTED: &str = "granted";

// Initialize logging for WASM
pub fn init_logging() {
    tracing_wasm::set_as_global_default();
    info!("WebSocket client initialized");
}

#[wasm_bindgen(js_name = initTracing)]
pub fn init_tracing() {
    init_logging();
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

#[derive(Clone, Default)]
struct OrientationReadingState {
    alpha: Option<f64>,
    beta: Option<f64>,
    gamma: Option<f64>,
    absolute: Option<bool>,
}

#[derive(Clone, Default)]
struct MotionReadingState {
    acceleration_x: Option<f64>,
    acceleration_y: Option<f64>,
    acceleration_z: Option<f64>,
    acceleration_including_gravity_x: Option<f64>,
    acceleration_including_gravity_y: Option<f64>,
    acceleration_including_gravity_z: Option<f64>,
    rotation_rate_alpha: Option<f64>,
    rotation_rate_beta: Option<f64>,
    rotation_rate_gamma: Option<f64>,
    interval_ms: Option<f64>,
}

#[wasm_bindgen]
pub struct OrientationReading {
    inner: OrientationReadingState,
}

#[wasm_bindgen]
pub struct MotionReading {
    inner: MotionReadingState,
}

#[wasm_bindgen]
pub struct DeviceSensors {
    active: bool,
    orientation_state: Rc<RefCell<Option<OrientationReadingState>>>,
    motion_state: Rc<RefCell<Option<MotionReadingState>>>,
    orientation_listener: Option<Closure<dyn FnMut(Event)>>,
    motion_listener: Option<Closure<dyn FnMut(Event)>>,
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
impl OrientationReading {
    pub fn alpha(&self) -> f64 {
        self.inner.alpha.unwrap_or(0.0)
    }

    pub fn beta(&self) -> f64 {
        self.inner.beta.unwrap_or(0.0)
    }

    pub fn gamma(&self) -> f64 {
        self.inner.gamma.unwrap_or(0.0)
    }

    pub fn absolute(&self) -> bool {
        self.inner.absolute.unwrap_or(false)
    }
}

#[wasm_bindgen]
impl MotionReading {
    #[wasm_bindgen(js_name = accelerationX)]
    pub fn acceleration_x(&self) -> f64 {
        self.inner.acceleration_x.unwrap_or(0.0)
    }

    #[wasm_bindgen(js_name = accelerationY)]
    pub fn acceleration_y(&self) -> f64 {
        self.inner.acceleration_y.unwrap_or(0.0)
    }

    #[wasm_bindgen(js_name = accelerationZ)]
    pub fn acceleration_z(&self) -> f64 {
        self.inner.acceleration_z.unwrap_or(0.0)
    }

    #[wasm_bindgen(js_name = accelerationIncludingGravityX)]
    pub fn acceleration_including_gravity_x(&self) -> f64 {
        self.inner.acceleration_including_gravity_x.unwrap_or(0.0)
    }

    #[wasm_bindgen(js_name = accelerationIncludingGravityY)]
    pub fn acceleration_including_gravity_y(&self) -> f64 {
        self.inner.acceleration_including_gravity_y.unwrap_or(0.0)
    }

    #[wasm_bindgen(js_name = accelerationIncludingGravityZ)]
    pub fn acceleration_including_gravity_z(&self) -> f64 {
        self.inner.acceleration_including_gravity_z.unwrap_or(0.0)
    }

    #[wasm_bindgen(js_name = rotationRateAlpha)]
    pub fn rotation_rate_alpha(&self) -> f64 {
        self.inner.rotation_rate_alpha.unwrap_or(0.0)
    }

    #[wasm_bindgen(js_name = rotationRateBeta)]
    pub fn rotation_rate_beta(&self) -> f64 {
        self.inner.rotation_rate_beta.unwrap_or(0.0)
    }

    #[wasm_bindgen(js_name = rotationRateGamma)]
    pub fn rotation_rate_gamma(&self) -> f64 {
        self.inner.rotation_rate_gamma.unwrap_or(0.0)
    }

    #[wasm_bindgen(js_name = intervalMs)]
    pub fn interval_ms(&self) -> f64 {
        self.inner.interval_ms.unwrap_or(0.0)
    }
}

impl Default for DeviceSensors {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl DeviceSensors {
    #[wasm_bindgen(constructor)]
    pub fn new() -> DeviceSensors {
        DeviceSensors {
            active: false,
            orientation_state: Rc::new(RefCell::new(None)),
            motion_state: Rc::new(RefCell::new(None)),
            orientation_listener: None,
            motion_listener: None,
        }
    }

    pub async fn start(&mut self) -> Result<(), JsValue> {
        if self.active {
            return Ok(());
        }

        let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;

        if js_sys::Reflect::get(&window, &JsValue::from_str("DeviceOrientationEvent"))?.is_undefined()
            && js_sys::Reflect::get(&window, &JsValue::from_str("DeviceMotionEvent"))?.is_undefined()
        {
            return Err(JsValue::from_str(
                "Device orientation and motion APIs are not supported in this browser.",
            ));
        }

        let orientation_permission = request_sensor_permission(js_sys::Reflect::get(
            &window,
            &JsValue::from_str("DeviceOrientationEvent"),
        )?)
        .await?;
        let motion_permission =
            request_sensor_permission(js_sys::Reflect::get(&window, &JsValue::from_str("DeviceMotionEvent"))?).await?;

        if orientation_permission != SENSOR_PERMISSION_GRANTED || motion_permission != SENSOR_PERMISSION_GRANTED {
            return Err(JsValue::from_str(&format!(
                "Sensor permission denied (orientation={orientation_permission}, motion={motion_permission})"
            )));
        }

        *self.orientation_state.borrow_mut() = None;
        *self.motion_state.borrow_mut() = None;

        let orientation_state = self.orientation_state.clone();
        let orientation_listener = Closure::wrap(Box::new(move |event: Event| {
            let value: JsValue = event.into();
            *orientation_state.borrow_mut() = Some(OrientationReadingState {
                alpha: js_number_field(&value, "alpha"),
                beta: js_number_field(&value, "beta"),
                gamma: js_number_field(&value, "gamma"),
                absolute: js_bool_field(&value, "absolute"),
            });
        }) as Box<dyn FnMut(Event)>);

        let motion_state = self.motion_state.clone();
        let motion_listener = Closure::wrap(Box::new(move |event: Event| {
            let value: JsValue = event.into();
            let acceleration = js_nested_object(&value, "acceleration");
            let acceleration_including_gravity = js_nested_object(&value, "accelerationIncludingGravity");
            let rotation_rate = js_nested_object(&value, "rotationRate");

            *motion_state.borrow_mut() = Some(MotionReadingState {
                acceleration_x: acceleration.as_ref().and_then(|v| js_number_field(v, "x")),
                acceleration_y: acceleration.as_ref().and_then(|v| js_number_field(v, "y")),
                acceleration_z: acceleration.as_ref().and_then(|v| js_number_field(v, "z")),
                acceleration_including_gravity_x: acceleration_including_gravity
                    .as_ref()
                    .and_then(|v| js_number_field(v, "x")),
                acceleration_including_gravity_y: acceleration_including_gravity
                    .as_ref()
                    .and_then(|v| js_number_field(v, "y")),
                acceleration_including_gravity_z: acceleration_including_gravity
                    .as_ref()
                    .and_then(|v| js_number_field(v, "z")),
                rotation_rate_alpha: rotation_rate.as_ref().and_then(|v| js_number_field(v, "alpha")),
                rotation_rate_beta: rotation_rate.as_ref().and_then(|v| js_number_field(v, "beta")),
                rotation_rate_gamma: rotation_rate.as_ref().and_then(|v| js_number_field(v, "gamma")),
                interval_ms: js_number_field(&value, "interval"),
            });
        }) as Box<dyn FnMut(Event)>);

        let target: &web_sys::EventTarget = window.as_ref();
        target.add_event_listener_with_callback("deviceorientation", orientation_listener.as_ref().unchecked_ref())?;
        target.add_event_listener_with_callback("devicemotion", motion_listener.as_ref().unchecked_ref())?;

        self.orientation_listener = Some(orientation_listener);
        self.motion_listener = Some(motion_listener);
        self.active = true;
        info!("Device sensors started");
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), JsValue> {
        if !self.active {
            return Ok(());
        }

        let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
        let target: &web_sys::EventTarget = window.as_ref();

        if let Some(listener) = self.orientation_listener.as_ref() {
            target.remove_event_listener_with_callback("deviceorientation", listener.as_ref().unchecked_ref())?;
        }

        if let Some(listener) = self.motion_listener.as_ref() {
            target.remove_event_listener_with_callback("devicemotion", listener.as_ref().unchecked_ref())?;
        }

        self.orientation_listener = None;
        self.motion_listener = None;
        self.active = false;
        info!("Device sensors stopped");
        Ok(())
    }

    #[wasm_bindgen(js_name = isActive)]
    pub fn is_active(&self) -> bool {
        self.active
    }

    #[wasm_bindgen(js_name = hasOrientation)]
    pub fn has_orientation(&self) -> bool {
        self.orientation_state.borrow().is_some()
    }

    #[wasm_bindgen(js_name = hasMotion)]
    pub fn has_motion(&self) -> bool {
        self.motion_state.borrow().is_some()
    }

    #[wasm_bindgen(js_name = orientationSnapshot)]
    pub fn orientation_snapshot(&self) -> Result<OrientationReading, JsValue> {
        self.orientation_state
            .borrow()
            .clone()
            .map(|inner| OrientationReading { inner })
            .ok_or_else(|| JsValue::from_str("No orientation reading available yet"))
    }

    #[wasm_bindgen(js_name = motionSnapshot)]
    pub fn motion_snapshot(&self) -> Result<MotionReading, JsValue> {
        self.motion_state
            .borrow()
            .clone()
            .map(|inner| MotionReading { inner })
            .ok_or_else(|| JsValue::from_str("No motion reading available yet"))
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
    if value.is_empty() { "unknown".to_string() } else { value }
}

async fn request_sensor_permission(target: JsValue) -> Result<String, JsValue> {
    if target.is_null() || target.is_undefined() {
        return Ok(SENSOR_PERMISSION_GRANTED.to_string());
    }

    let request_permission = js_sys::Reflect::get(&target, &JsValue::from_str("requestPermission"))?;
    if request_permission.is_null() || request_permission.is_undefined() {
        return Ok(SENSOR_PERMISSION_GRANTED.to_string());
    }

    let request_permission = request_permission
        .dyn_into::<js_sys::Function>()
        .map_err(|_| JsValue::from_str("requestPermission is not callable"))?;
    let promise = request_permission
        .call0(&target)?
        .dyn_into::<js_sys::Promise>()
        .map_err(|_| JsValue::from_str("requestPermission did not return a Promise"))?;
    let result = JsFuture::from(promise).await?;
    Ok(result
        .as_string()
        .unwrap_or_else(|| SENSOR_PERMISSION_GRANTED.to_string()))
}

fn js_number_field(value: &JsValue, field: &str) -> Option<f64> {
    js_sys::Reflect::get(value, &JsValue::from_str(field))
        .ok()
        .and_then(|field_value| field_value.as_f64())
}

fn js_bool_field(value: &JsValue, field: &str) -> Option<bool> {
    js_sys::Reflect::get(value, &JsValue::from_str(field))
        .ok()
        .and_then(|field_value| field_value.as_bool())
}

fn js_nested_object(value: &JsValue, field: &str) -> Option<JsValue> {
    js_sys::Reflect::get(value, &JsValue::from_str(field))
        .ok()
        .filter(|nested| !nested.is_null() && !nested.is_undefined())
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
            alive_interval_ms: DEFAULT_ALIVE_INTERVAL_MS,
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
    alive_interval_id: Option<i32>,
    reconnect_timeout_id: Option<i32>,
    offline_queue: VecDeque<String>,
    manual_disconnect: bool,
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
            alive_interval_id: None,
            reconnect_timeout_id: None,
            offline_queue: VecDeque::with_capacity(MAX_OFFLINE_QUEUE_LEN),
            manual_disconnect: false,
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
            s.manual_disconnect = false;
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
                    if let Some(timeout_id) = s.reconnect_timeout_id.take()
                        && let Some(window) = web_sys::window()
                    {
                        window.clear_timeout_with_handle(timeout_id);
                    }
                }
                cli_ptr.notify_state_change();
                if let Err(error) = cli_ptr.send_connect_message() {
                    error!("Failed to send connect message: {:?}", error);
                }
                cli_ptr.flush_offline_queue();
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
                            WsMessage::ListAgents => {
                                warn!("Unexpected list_agents message from server");
                            }
                            WsMessage::ListAgentsResponse { agents } => {
                                info!("Server returned {} agents", agents.len());
                            }
                            WsMessage::SendAgentMessage { .. } => {
                                warn!("Unexpected send_agent_message request from server");
                            }
                            WsMessage::BroadcastMessage { .. } => {
                                warn!("Unexpected broadcast_message request from server");
                            }
                            WsMessage::AgentMessage {
                                message_id,
                                from_agent_id,
                                scope,
                                server_received_at,
                                ..
                            } => {
                                info!(
                                    "Received {:?} agent message {} from {} at {}",
                                    scope, message_id, from_agent_id, server_received_at
                                );
                            }
                            WsMessage::MessageAck { .. } => {
                                warn!("Unexpected message_ack from server");
                            }
                            WsMessage::MessageStatus {
                                message_id,
                                status,
                                detail,
                            } => {
                                info!("Message status update {:?} {:?}: {}", message_id, status, detail);
                            }
                            WsMessage::Invalid { message_id, detail } => {
                                warn!("Invalid server message {:?}: {}", message_id, detail);
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
                            WsMessage::StoreFile { .. } | WsMessage::FetchFile { .. } => {
                                warn!("Unexpected file storage request from server");
                            }
                        }
                    }
                    // Notify callback if set
                    let s = shared.borrow();
                    if let Some(ref callback) = s.on_message_callback
                        && let Some(function) = callback.dyn_ref::<js_sys::Function>()
                    {
                        let _ = function.call1(&JsValue::NULL, &JsValue::from_str(&data));
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
        self.cancel_reconnect();
        self.record_offline();
        {
            let mut s = self.shared.borrow_mut();
            s.manual_disconnect = true;
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
        let should_queue = {
            let s = self.shared.borrow();
            s.state != ConnectionState::Connected || s.socket.is_none()
        };

        if should_queue {
            self.enqueue_offline_message(message);
            return Ok(());
        }

        let send_result = {
            let s = self.shared.borrow();
            s.socket
                .as_ref()
                .ok_or_else(|| JsValue::from_str("No websocket available"))?
                .send_with_str(message)
        };

        match send_result {
            Ok(()) => {
                info!("Message sent: {}", message);
                Ok(())
            }
            Err(error) => {
                warn!("Send failed while online, queueing message for retry: {:?}", error);
                self.enqueue_offline_message(message);
                Err(JsValue::from_str(&format!(
                    "Failed to send message immediately; queued for retry: {:?}",
                    error
                )))
            }
        }
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
        self.stop_alive_interval();

        let Some(window) = web_sys::window() else {
            warn!("No window available to start alive interval");
            return;
        };

        let interval_ms = self.config.alive_interval_ms as i32;
        let cli_ptr = self.clone();
        let interval_closure = Closure::wrap(Box::new(move || {
            if let Err(error) = cli_ptr.send_alive() {
                warn!("Failed to send alive keepalive: {:?}", error);
            }
        }) as Box<dyn FnMut()>);

        match window.set_interval_with_callback_and_timeout_and_arguments_0(
            interval_closure.as_ref().unchecked_ref(),
            interval_ms,
        ) {
            Ok(interval_id) => {
                self.shared.borrow_mut().alive_interval_id = Some(interval_id);
                info!("Started alive interval at {}ms", self.config.alive_interval_ms);
                interval_closure.forget();
            }
            Err(error) => {
                warn!("Failed to start alive interval: {:?}", error);
            }
        }
    }

    fn stop_alive_interval(&self) {
        let mut s = self.shared.borrow_mut();
        if let Some(interval_id) = s.alive_interval_id.take() {
            if let Some(window) = web_sys::window() {
                window.clear_interval_with_handle(interval_id);
            }
            info!("Stopped alive interval");
        }
    }

    fn handle_disconnect(&mut self) {
        self.stop_alive_interval();
        let manual_disconnect = {
            let mut s = self.shared.borrow_mut();
            s.socket = None;
            s.state = ConnectionState::Disconnected;
            s.manual_disconnect
        };
        self.record_offline();
        self.notify_state_change();

        if manual_disconnect {
            info!("Manual websocket disconnect; skipping reconnect");
            return;
        }

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
            self.schedule_reconnect(next_delay as i32);
        } else {
            error!("Max reconnection attempts reached");
        }
    }

    fn notify_state_change(&self) {
        let state = self.get_state();
        let s = self.shared.borrow();
        if let Some(ref callback) = s.on_state_change_callback
            && let Some(function) = callback.dyn_ref::<js_sys::Function>()
        {
            let _ = function.call1(&JsValue::NULL, &JsValue::from_str(&state));
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

    fn enqueue_offline_message(&self, message: &str) {
        let mut s = self.shared.borrow_mut();
        if s.offline_queue.len() == MAX_OFFLINE_QUEUE_LEN {
            s.offline_queue.pop_front();
            warn!(
                "Offline websocket queue reached {} messages; dropping oldest entry",
                MAX_OFFLINE_QUEUE_LEN
            );
        }
        s.offline_queue.push_back(message.to_string());
        info!(
            "Queued websocket message while offline (queue_len={}): {}",
            s.offline_queue.len(),
            message
        );
    }

    fn flush_offline_queue(&self) {
        loop {
            let next_message = {
                let mut s = self.shared.borrow_mut();
                if s.state != ConnectionState::Connected || s.socket.is_none() {
                    return;
                }
                s.offline_queue.pop_front()
            };

            let Some(message) = next_message else {
                return;
            };

            let send_result = {
                let s = self.shared.borrow();
                s.socket
                    .as_ref()
                    .ok_or_else(|| JsValue::from_str("No websocket available"))
                    .and_then(|socket| {
                        socket
                            .send_with_str(&message)
                            .map_err(|error| JsValue::from_str(&format!("Failed to flush queued message: {:?}", error)))
                    })
            };

            if let Err(error) = send_result {
                warn!("Failed to flush queued websocket message; re-queueing: {:?}", error);
                let mut s = self.shared.borrow_mut();
                s.offline_queue.push_front(message);
                return;
            }

            info!("Flushed queued websocket message: {}", message);
        }
    }

    fn record_offline(&self) {
        let timestamp = chrono::Utc::now().to_rfc3339();
        match store_last_offline_at(&timestamp) {
            Ok(()) => info!("Recorded websocket offline transition at {}", timestamp),
            Err(error) => warn!("Failed to record websocket offline transition: {:?}", error),
        }
    }

    fn schedule_reconnect(&self, delay_ms: i32) {
        self.cancel_reconnect();

        let Some(window) = web_sys::window() else {
            warn!("No window available to schedule reconnect");
            return;
        };

        let mut cli_ptr = self.clone();
        let reconnect_closure = Closure::once(Box::new(move || {
            if let Err(error) = cli_ptr.connect() {
                error!("Reconnect attempt failed: {:?}", error);
            }
        }) as Box<dyn FnOnce()>);

        match window
            .set_timeout_with_callback_and_timeout_and_arguments_0(reconnect_closure.as_ref().unchecked_ref(), delay_ms)
        {
            Ok(timeout_id) => {
                self.shared.borrow_mut().reconnect_timeout_id = Some(timeout_id);
                reconnect_closure.forget();
            }
            Err(error) => {
                warn!("Failed to schedule reconnect: {:?}", error);
            }
        }
    }

    fn cancel_reconnect(&self) {
        let mut s = self.shared.borrow_mut();
        if let Some(timeout_id) = s.reconnect_timeout_id.take()
            && let Some(window) = web_sys::window()
        {
            window.clear_timeout_with_handle(timeout_id);
        }
    }
}

impl WsClient {
    pub fn request_list_agents(&self) -> Result<(), JsValue> {
        let payload = serde_json::to_string(&WsMessage::ListAgents)
            .map_err(|error| JsValue::from_str(&format!("Failed to serialize list_agents: {error}")))?;
        self.send(&payload)
    }

    pub fn broadcast_message(&self, message: serde_json::Value) -> Result<(), JsValue> {
        let payload = serde_json::to_string(&WsMessage::BroadcastMessage { message })
            .map_err(|error| JsValue::from_str(&format!("Failed to serialize broadcast message: {error}")))?;
        self.send(&payload)
    }

    pub fn send_agent_message(
        &self,
        to_agent_id: impl Into<String>,
        message: serde_json::Value,
    ) -> Result<(), JsValue> {
        let payload = serde_json::to_string(&WsMessage::SendAgentMessage {
            to_agent_id: to_agent_id.into(),
            message,
        })
        .map_err(|error| JsValue::from_str(&format!("Failed to serialize direct message: {error}")))?;
        self.send(&payload)
    }

    pub fn send_client_event(
        &self,
        capability: impl Into<String>,
        action: impl Into<String>,
        details: serde_json::Value,
    ) -> Result<(), JsValue> {
        let message = WsMessage::ClientEvent {
            capability: capability.into(),
            action: action.into(),
            details,
        };
        let payload = serde_json::to_string(&message)
            .map_err(|error| JsValue::from_str(&format!("Failed to serialize client event: {error}")))?;
        self.send(&payload)
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

fn store_last_offline_at(timestamp: &str) -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
    let storage = window
        .local_storage()?
        .ok_or_else(|| JsValue::from_str("No localStorage available"))?;
    storage.set_item(STORED_LAST_OFFLINE_AT_KEY, timestamp)
}

#[wasm_bindgen(js_name = set_textarea_value)]
pub fn set_textarea_value(element_id: &str, message: &str) -> Result<(), JsValue> {
    if let Some(window) = web_sys::window()
        && let Some(document) = window.document()
        && let Some(output) = document.get_element_by_id(element_id)
    {
        js_sys::Reflect::set(
            output.as_ref(),
            &JsValue::from_str("value"),
            &JsValue::from_str(message),
        )?;
    }

    Ok(())
}

#[wasm_bindgen(js_name = append_to_textarea)]
pub fn append_to_textarea(element_id: &str, message: &str) -> Result<(), JsValue> {
    if let Some(window) = web_sys::window()
        && let Some(document) = window.document()
        && let Some(output) = document.get_element_by_id(element_id)
    {
        let current_value = js_sys::Reflect::get(output.as_ref(), &JsValue::from_str("value"))?
            .as_string()
            .unwrap_or_default();
        let next_value = if current_value.is_empty() || current_value.starts_with("Workflow module") {
            message.to_string()
        } else {
            format!("{current_value}\n{message}")
        };

        js_sys::Reflect::set(
            output.as_ref(),
            &JsValue::from_str("value"),
            &JsValue::from_str(&next_value),
        )?;

        // Auto-scroll to bottom
        js_sys::Reflect::set(
            output.as_ref(),
            &JsValue::from_str("scrollTop"),
            &js_sys::Reflect::get(output.as_ref(), &JsValue::from_str("scrollHeight"))?,
        )?;
    }

    Ok(())
}
