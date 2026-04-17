use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use et_web::{SENSOR_PERMISSION_GRANTED, request_sensor_permission};
use et_ws_wasm_agent::{
    WsClient, WsClientConfig, js_bool_field, js_nested_object, js_number_field, set_textarea_value,
};
use js_sys::{Array, Float32Array, Function, Promise, Reflect};
use serde_json::json;
use tracing::info;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::Event;

const HAR_MODEL_PATH: &str = "/static/models/human_activity_recognition.onnx";
const HAR_SEQUENCE_LENGTH: usize = 512;
const HAR_FEATURE_COUNT: usize = 9;
const HAR_SAMPLE_INTERVAL_MS: i32 = 20;
const HAR_INFERENCE_INTERVAL_MS: f64 = 250.0;
const STANDARD_GRAVITY: f64 = 9.80665;
const GRAVITY_FILTER_ALPHA: f64 = 0.8;
const HAR_CLASS_LABELS: [&str; 6] = ["class_0", "class_1", "class_2", "class_3", "class_4", "class_5"];

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
pub struct MotionReading {
    inner: MotionReadingState,
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

#[wasm_bindgen]
pub struct DeviceSensors {
    active: bool,
    orientation_state: Rc<RefCell<Option<OrientationReadingState>>>,
    motion_state: Rc<RefCell<Option<MotionReadingState>>>,
    orientation_listener: Option<Closure<dyn FnMut(Event)>>,
    motion_listener: Option<Closure<dyn FnMut(Event)>>,
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

#[wasm_bindgen(start)]
pub fn init() {
    let _ = tracing_wasm::try_set_as_global_default();
    info!("har1 workflow module initialized");
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
pub async fn run() -> Result<(), JsValue> {
    set_har_status("har1: entered run()")?;
    log("entered run()")?;
    log("using existing tracing setup")?;

    let outcome = async {
        let ws_url = websocket_url()?;
        set_har_status(&format!("har1: resolved websocket URL\n{ws_url}"))?;
        log(&format!("resolved websocket URL: {ws_url}"))?;
        let mut client = WsClient::new(WsClientConfig::new(ws_url));
        log("connecting websocket client")?;
        client.connect()?;
        wait_for_connected(&client).await?;
        log(&format!("websocket connected with agent_id={}", client.get_client_id()))?;

        let mut sensors = DeviceSensors::new();
        log("starting har1 workflow")?;

        let result = run_inner(&client, &mut sensors).await;
        let stop_result = sensors.stop();
        client.disconnect();

        match (result, stop_result) {
            (Ok(()), Ok(())) => {
                log("har1 workflow finished")?;
                Ok(())
            }
            (Err(error), Ok(())) => Err(error),
            (Ok(()), Err(error)) => Err(error),
            (Err(error), Err(_)) => Err(error),
        }
    }
    .await;

    if let Err(error) = &outcome {
        let message = describe_js_error(error);
        let _ = set_har_status(&format!("har1: error\n{message}"));
        let _ = log(&format!("error: {message}"));
    }

    outcome
}

async fn run_inner(client: &WsClient, sensors: &mut DeviceSensors) -> Result<(), JsValue> {
    set_har_status("har1: loading HAR model")?;
    log(&format!("loading HAR model from {HAR_MODEL_PATH}"))?;
    let session = create_har_session(HAR_MODEL_PATH).await?;
    let input_name = first_string_entry(&session, "inputNames")?;
    let output_name = first_string_entry(&session, "outputNames")?;

    set_har_status(&format!(
        "har1: HAR model loaded\npath: {HAR_MODEL_PATH}\ninput: {input_name}\noutput: {output_name}"
    ))?;
    log(&format!("HAR model loaded: input={input_name} output={output_name}"))?;

    log("requesting sensor access")?;
    sensors.start().await?;
    log("sensors started")?;
    render_sensor_output(sensors)?;
    log("waiting for first motion sample")?;
    wait_for_motion_sample(sensors).await?;
    log("first motion sample received")?;

    let mut gravity_estimate = [0.0_f64; 3];
    let mut sample_buffer: VecDeque<[f32; HAR_FEATURE_COUNT]> = VecDeque::with_capacity(HAR_SEQUENCE_LENGTH);
    let mut last_inference_at = 0.0_f64;
    let mut last_class_label: Option<String> = None;
    let mut class_change_count = 0_u32;

    while class_change_count < 3 {
        sleep_ms(HAR_SAMPLE_INTERVAL_MS).await?;

        if !sensors.has_motion() {
            continue;
        }

        let reading = sensors.motion_snapshot()?;
        render_sensor_output(sensors)?;
        sample_buffer.push_back(feature_vector(&reading, &mut gravity_estimate));
        if sample_buffer.len() > HAR_SEQUENCE_LENGTH {
            sample_buffer.pop_front();
        }

        if sample_buffer.len() == 1
            || sample_buffer.len() == 64
            || sample_buffer.len() == 128
            || sample_buffer.len() == 256
        {
            log(&format!(
                "buffering HAR samples: {}/{}",
                sample_buffer.len(),
                HAR_SEQUENCE_LENGTH
            ))?;
        }

        if sample_buffer.len() < HAR_SEQUENCE_LENGTH {
            continue;
        }

        if sample_buffer.len() == HAR_SEQUENCE_LENGTH {
            set_har_status("har1: HAR sample window full; inference loop active")?;
            log("HAR sample window full; starting inference loop")?;
        }

        let now = js_sys::Date::now();
        if now - last_inference_at < HAR_INFERENCE_INTERVAL_MS {
            continue;
        }
        last_inference_at = now;

        let prediction = infer_prediction(&session, &input_name, &output_name, &sample_buffer).await?;
        if last_class_label.as_deref() == Some(prediction.best_label.as_str()) {
            continue;
        }

        class_change_count += 1;
        client.send_client_event(
            "har",
            "class_changed",
            json!({
                "detected_class": prediction.best_label,
                "previous_class": last_class_label,
                "class_index": prediction.best_index,
                "confidence": prediction.best_probability,
                "probabilities": prediction.probabilities,
                "logits": prediction.logits,
                "buffered_samples": sample_buffer.len(),
                "detected_at": String::from(js_sys::Date::new_0().to_iso_string()),
            }),
        )?;

        log(&format!(
            "class change {} of 3: {} -> {}",
            class_change_count,
            last_class_label.as_deref().unwrap_or("none"),
            prediction.best_label
        ))?;
        set_har_status(&format!(
            "har1: inference running\nlatest class: {}\nclass changes: {}/3\nbuffered samples: {}",
            prediction.best_label,
            class_change_count,
            sample_buffer.len()
        ))?;
        last_class_label = Some(prediction.best_label);
    }

    set_har_status("har1: workflow complete")?;
    Ok(())
}

fn render_sensor_output(sensors: &DeviceSensors) -> Result<(), JsValue> {
    let orientation = if sensors.has_orientation() {
        Some(sensors.orientation_snapshot()?)
    } else {
        None
    };
    let motion = if sensors.has_motion() {
        Some(sensors.motion_snapshot()?)
    } else {
        None
    };

    let mut lines = vec![
        String::from("Device sensor stream"),
        format!(
            "updated: {}",
            String::from(js_sys::Date::new_0().to_locale_time_string("en-US"))
        ),
        String::new(),
        String::from("orientation"),
    ];

    if let Some(orientation) = orientation {
        lines.push(format!("alpha: {}", format_number(orientation.alpha(), 3)));
        lines.push(format!("beta: {}", format_number(orientation.beta(), 3)));
        lines.push(format!("gamma: {}", format_number(orientation.gamma(), 3)));
        lines.push(format!("absolute: {}", orientation.absolute()));
    } else {
        lines.push(String::from("waiting for orientation event..."));
    }

    lines.push(String::new());
    lines.push(String::from("motion"));
    if let Some(motion) = motion {
        lines.push(format!(
            "acceleration: x={} y={} z={}",
            format_number(motion.acceleration_x(), 3),
            format_number(motion.acceleration_y(), 3),
            format_number(motion.acceleration_z(), 3)
        ));
        lines.push(format!(
            "acceleration including gravity: x={} y={} z={}",
            format_number(motion.acceleration_including_gravity_x(), 3),
            format_number(motion.acceleration_including_gravity_y(), 3),
            format_number(motion.acceleration_including_gravity_z(), 3)
        ));
        lines.push(format!(
            "rotation rate: alpha={} beta={} gamma={}",
            format_number(motion.rotation_rate_alpha(), 3),
            format_number(motion.rotation_rate_beta(), 3),
            format_number(motion.rotation_rate_gamma(), 3)
        ));
        lines.push(format!("interval: {} ms", format_number(motion.interval_ms(), 1)));
    } else {
        lines.push(String::from("waiting for motion event..."));
    }

    set_textarea_value("sensor-output", &lines.join("\n"))
}

struct Prediction {
    best_index: usize,
    best_label: String,
    best_probability: f64,
    probabilities: Vec<f64>,
    logits: Vec<f64>,
}

fn log(message: &str) -> Result<(), JsValue> {
    let line = format!("[har1] {message}");
    web_sys::console::log_1(&JsValue::from_str(&line));

    if let Some(window) = web_sys::window()
        && let Some(document) = window.document()
        && let Some(log_el) = document.get_element_by_id("log")
    {
        let current = log_el.text_content().unwrap_or_default();
        let next = if current.is_empty() {
            line
        } else {
            format!("{current}\n{line}")
        };
        log_el.set_text_content(Some(&next));
    }

    Ok(())
}

fn set_har_status(message: &str) -> Result<(), JsValue> {
    set_textarea_value("module-output", message)
}

fn format_number(value: f64, digits: usize) -> String {
    if value.is_finite() {
        format!("{value:.digits$}")
    } else {
        String::from("n/a")
    }
}

fn describe_js_error(error: &JsValue) -> String {
    error
        .as_string()
        .or_else(|| js_sys::JSON::stringify(error).ok().map(String::from))
        .unwrap_or_else(|| format!("{error:?}"))
}

fn method(target: &JsValue, name: &str) -> Result<Function, JsValue> {
    Reflect::get(target, &JsValue::from_str(name))?
        .dyn_into::<Function>()
        .map_err(|_| JsValue::from_str(&format!("{name} is not callable")))
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

async fn wait_for_motion_sample(sensors: &DeviceSensors) -> Result<(), JsValue> {
    for _ in 0..100 {
        if sensors.has_motion() {
            return Ok(());
        }
        sleep_ms(100).await?;
    }

    Err(JsValue::from_str("Timed out waiting for initial motion sample"))
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
    Ok(format!("{ws_protocol}//{host}/ws"))
}

async fn create_har_session(model_path: &str) -> Result<JsValue, JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
    let ort = Reflect::get(window.as_ref(), &JsValue::from_str("ort"))?;
    if ort.is_null() || ort.is_undefined() {
        return Err(JsValue::from_str("onnxruntime-web did not load"));
    }

    configure_onnx_runtime_wasm(&window, &ort)?;

    let inference_session = Reflect::get(&ort, &JsValue::from_str("InferenceSession"))?;
    let create = method(&inference_session, "create")?;
    let options = js_sys::Object::new();
    Reflect::set(
        &options,
        &JsValue::from_str("executionProviders"),
        &Array::of1(&JsValue::from_str("wasm")),
    )?;

    let value = create.call2(&inference_session, &JsValue::from_str(model_path), &options)?;
    JsFuture::from(
        value
            .dyn_into::<Promise>()
            .map_err(|_| JsValue::from_str("InferenceSession.create did not return a Promise"))?,
    )
    .await
}

fn configure_onnx_runtime_wasm(window: &web_sys::Window, ort: &JsValue) -> Result<(), JsValue> {
    let env = Reflect::get(ort, &JsValue::from_str("env"))?;
    let wasm = Reflect::get(&env, &JsValue::from_str("wasm"))?;
    if wasm.is_null() || wasm.is_undefined() {
        return Err(JsValue::from_str("onnxruntime-web environment is unavailable"));
    }

    let versions = Reflect::get(&env, &JsValue::from_str("versions"))?;
    let ort_version = Reflect::get(&versions, &JsValue::from_str("web"))?
        .as_string()
        .ok_or_else(|| JsValue::from_str("onnxruntime-web version is unavailable"))?;
    let dist_base_url = format!("https://cdn.jsdelivr.net/npm/onnxruntime-web@{ort_version}/dist");

    let supports_threads = Reflect::get(window.as_ref(), &JsValue::from_str("crossOriginIsolated"))?
        .as_bool()
        .unwrap_or(false)
        && Reflect::has(window.as_ref(), &JsValue::from_str("SharedArrayBuffer"))?;

    Reflect::set(
        &wasm,
        &JsValue::from_str("numThreads"),
        &JsValue::from_f64(if supports_threads { 0.0 } else { 1.0 }),
    )?;

    let wasm_paths = js_sys::Object::new();
    Reflect::set(
        &wasm_paths,
        &JsValue::from_str("mjs"),
        &JsValue::from_str(&format!("{dist_base_url}/ort-wasm-simd-threaded.mjs")),
    )?;
    Reflect::set(
        &wasm_paths,
        &JsValue::from_str("wasm"),
        &JsValue::from_str(&format!("{dist_base_url}/ort-wasm-simd-threaded.wasm")),
    )?;
    Reflect::set(&wasm, &JsValue::from_str("wasmPaths"), &wasm_paths)?;
    Ok(())
}

fn first_string_entry(target: &JsValue, field: &str) -> Result<String, JsValue> {
    let values = Reflect::get(target, &JsValue::from_str(field))?;
    let first = Reflect::get(&values, &JsValue::from_f64(0.0))?;
    first
        .as_string()
        .ok_or_else(|| JsValue::from_str(&format!("Missing first entry for {field}")))
}

async fn infer_prediction(
    session: &JsValue,
    input_name: &str,
    output_name: &str,
    sample_buffer: &VecDeque<[f32; HAR_FEATURE_COUNT]>,
) -> Result<Prediction, JsValue> {
    let flat_samples = flatten_samples(sample_buffer);
    let tensor = create_tensor(&flat_samples)?;
    let feeds = js_sys::Object::new();
    Reflect::set(&feeds, &JsValue::from_str(input_name), &tensor)?;

    let run_value = method(session, "run")?.call1(session, &feeds)?;
    let result = JsFuture::from(
        run_value
            .dyn_into::<Promise>()
            .map_err(|_| JsValue::from_str("InferenceSession.run did not return a Promise"))?,
    )
    .await?;

    let output = Reflect::get(&result, &JsValue::from_str(output_name))?;
    let data = Reflect::get(&output, &JsValue::from_str("data"))?;
    let logits_f32 = Float32Array::new(&data).to_vec();
    let logits: Vec<f64> = logits_f32.into_iter().map(f64::from).collect();
    let probabilities = softmax(&logits);
    let (best_index, best_probability) = probabilities
        .iter()
        .copied()
        .enumerate()
        .max_by(|(_, left), (_, right)| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal))
        .ok_or_else(|| JsValue::from_str("Model returned no prediction scores"))?;
    let best_label = HAR_CLASS_LABELS
        .get(best_index)
        .copied()
        .unwrap_or("unknown")
        .to_string();

    Ok(Prediction {
        best_index,
        best_label,
        best_probability,
        probabilities,
        logits,
    })
}

fn create_tensor(values: &[f32]) -> Result<JsValue, JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
    let ort = Reflect::get(window.as_ref(), &JsValue::from_str("ort"))?;
    let tensor_ctor = Reflect::get(&ort, &JsValue::from_str("Tensor"))?
        .dyn_into::<Function>()
        .map_err(|_| JsValue::from_str("ort.Tensor is not callable"))?;

    let dims = Array::new();
    dims.push(&JsValue::from_f64(1.0));
    dims.push(&JsValue::from_f64(HAR_SEQUENCE_LENGTH as f64));
    dims.push(&JsValue::from_f64(HAR_FEATURE_COUNT as f64));

    let args = Array::new();
    args.push(&JsValue::from_str("float32"));
    args.push(&Float32Array::from(values).into());
    args.push(&dims.into());

    Reflect::construct(&tensor_ctor, &args)
}

fn flatten_samples(sample_buffer: &VecDeque<[f32; HAR_FEATURE_COUNT]>) -> Vec<f32> {
    sample_buffer.iter().flat_map(|sample| sample.iter().copied()).collect()
}

fn feature_vector(reading: &MotionReading, gravity_estimate: &mut [f64; 3]) -> [f32; HAR_FEATURE_COUNT] {
    let total_acceleration = [
        reading.acceleration_including_gravity_x(),
        reading.acceleration_including_gravity_y(),
        reading.acceleration_including_gravity_z(),
    ];

    for (index, value) in total_acceleration.iter().enumerate() {
        gravity_estimate[index] = GRAVITY_FILTER_ALPHA * gravity_estimate[index] + (1.0 - GRAVITY_FILTER_ALPHA) * value;
    }

    let body_acceleration = [
        total_acceleration[0] - gravity_estimate[0],
        total_acceleration[1] - gravity_estimate[1],
        total_acceleration[2] - gravity_estimate[2],
    ];

    [
        to_g(body_acceleration[0]) as f32,
        to_g(body_acceleration[1]) as f32,
        to_g(body_acceleration[2]) as f32,
        degrees_to_radians(reading.rotation_rate_beta()) as f32,
        degrees_to_radians(reading.rotation_rate_gamma()) as f32,
        degrees_to_radians(reading.rotation_rate_alpha()) as f32,
        to_g(total_acceleration[0]) as f32,
        to_g(total_acceleration[1]) as f32,
        to_g(total_acceleration[2]) as f32,
    ]
}

fn degrees_to_radians(value: f64) -> f64 {
    value * std::f64::consts::PI / 180.0
}

fn to_g(value: f64) -> f64 {
    value / STANDARD_GRAVITY
}

fn softmax(values: &[f64]) -> Vec<f64> {
    if values.is_empty() {
        return Vec::new();
    }

    let max_value = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let exps: Vec<f64> = values.iter().map(|value| (value - max_value).exp()).collect();
    let sum: f64 = exps.iter().sum();
    exps.into_iter().map(|value| value / sum).collect()
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

#[cfg(test)]
mod test_har1;
