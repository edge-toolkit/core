use std::collections::VecDeque;

use et_ws_wasm_agent::{DeviceSensors, MotionReading, WsClient, WsClientConfig};
use js_sys::{Array, Float32Array, Function, Promise, Reflect};
use serde_json::json;
use tracing::info;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

const HAR_MODEL_PATH: &str = "/static/models/human_activity_recognition.onnx";
const HAR_SEQUENCE_LENGTH: usize = 512;
const HAR_FEATURE_COUNT: usize = 9;
const HAR_SAMPLE_INTERVAL_MS: i32 = 20;
const HAR_INFERENCE_INTERVAL_MS: f64 = 250.0;
const STANDARD_GRAVITY: f64 = 9.80665;
const GRAVITY_FILTER_ALPHA: f64 = 0.8;
const HAR_CLASS_LABELS: [&str; 6] = ["class_0", "class_1", "class_2", "class_3", "class_4", "class_5"];

#[wasm_bindgen(start)]
pub fn init() {
    tracing_wasm::set_as_global_default();
    info!("har1 workflow module initialized");
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
    set_textarea_value("har-output", message)
}

fn set_textarea_value(element_id: &str, message: &str) -> Result<(), JsValue> {
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
