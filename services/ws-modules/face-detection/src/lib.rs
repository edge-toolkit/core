use std::cell::{Cell, RefCell};
use std::rc::Rc;

use et_web::get_media_devices;
use et_ws_wasm_agent::{WsClient, WsClientConfig, set_textarea_value};
use js_sys::{Array, Float32Array, Function, Promise, Reflect};
use serde_json::json;
use tracing::info;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::MediaStreamConstraints;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, HtmlVideoElement, ImageData, MediaStream};

const FACE_MODEL_PATH: &str = "/modules/et-model-face1/video_cv.onnx";
const FACE_INPUT_WIDTH: usize = 640;
const FACE_INPUT_HEIGHT: usize = 608;
const FACE_INPUT_WIDTH_F64: f64 = FACE_INPUT_WIDTH as f64;
const FACE_INPUT_HEIGHT_F64: f64 = FACE_INPUT_HEIGHT as f64;
const FACE_INFERENCE_INTERVAL_MS: i32 = 750;
const FACE_RENDER_INTERVAL_MS: i32 = 60;
const RETINAFACE_CONFIDENCE_THRESHOLD: f64 = 0.75;
const RETINAFACE_NMS_THRESHOLD: f64 = 0.4;
const RETINAFACE_VARIANCES: [f64; 2] = [0.1, 0.2];
const RETINAFACE_MIN_SIZES: [&[f64]; 3] = [&[16.0, 32.0], &[64.0, 128.0], &[256.0, 512.0]];
const RETINAFACE_STEPS: [f64; 3] = [8.0, 16.0, 32.0];

#[derive(Clone)]
struct Detection {
    label: String,
    class_index: i32,
    score: f64,
    box_coords: [f64; 4],
}

#[derive(Clone)]
struct DetectionSummary {
    detections: Vec<Detection>,
    confidence: f64,
    processed_at: String,
}

struct FaceCaptureTensor {
    data: Vec<f32>,
    resize_ratio: f64,
    source_width: f64,
    source_height: f64,
}

struct FaceDetectionRuntime {
    client: WsClient,
    capture: VideoCapture,
    inference_interval_id: i32,
    render_interval_id: i32,
    _inference_closure: Closure<dyn FnMut()>,
    _render_closure: Closure<dyn FnMut()>,
}

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

thread_local! {
    static FACE_RUNTIME: RefCell<Option<FaceDetectionRuntime>> = const { RefCell::new(None) };
    static FACE_PREPROCESS_CANVAS: RefCell<Option<HtmlCanvasElement>> = const { RefCell::new(None) };
}

#[wasm_bindgen(start)]
pub fn init() {
    let _ = tracing_wasm::try_set_as_global_default();
    info!("face detection workflow module initialized");
}

#[wasm_bindgen]
pub fn is_running() -> bool {
    FACE_RUNTIME.with(|runtime| runtime.borrow().is_some())
}

#[wasm_bindgen]
pub async fn run() -> Result<(), JsValue> {
    if is_running() {
        return Ok(());
    }

    face_set_status("face detection: starting");
    log(&format!("loading RetinaFace model from {FACE_MODEL_PATH}"))?;

    let ws_url = websocket_url()?;
    let mut client = WsClient::new(WsClientConfig::new(ws_url.clone()));
    client.connect()?;
    wait_for_connected(&client).await?;
    log(&format!("websocket connected with agent_id={}", client.get_client_id()))?;

    let capture = match VideoCapture::request().await {
        Ok(capture) => capture,
        Err(error) => {
            client.disconnect();
            return Err(error);
        }
    };

    if let Err(error) = face_attach_stream(capture.raw_stream()).await {
        capture.stop();
        client.disconnect();
        return Err(error);
    }

    let session = match create_face_session(FACE_MODEL_PATH).await {
        Ok(session) => session,
        Err(error) => {
            capture.stop();
            face_detach_stream();
            client.disconnect();
            return Err(error);
        }
    };

    let input_name = first_string_entry(&session, "inputNames")?;
    let output_names = string_entries(&session, "outputNames")?;
    if output_names.len() < 3 {
        capture.stop();
        face_detach_stream();
        client.disconnect();
        return Err(JsValue::from_str(
            "RetinaFace session did not expose the expected outputs",
        ));
    }

    let last_summary: Rc<RefCell<Option<DetectionSummary>>> = Rc::new(RefCell::new(None));
    let inference_pending = Rc::new(Cell::new(false));
    let last_has_detection = Rc::new(Cell::new(false));
    let inference_count = Rc::new(Cell::new(0));

    let inference_session = session.clone();
    let inference_input_name = input_name.clone();
    let inference_output_names = output_names.clone();
    let inference_client = client.clone();
    let inference_last_summary = last_summary.clone();
    let inference_pending_flag = inference_pending.clone();
    let inference_last_has_detection = last_has_detection.clone();
    let inference_count_ref = inference_count.clone();
    let inference_closure = Closure::wrap(Box::new(move || {
        if inference_pending_flag.get() {
            return;
        }

        if inference_count_ref.get() >= 20 {
            return;
        }

        inference_pending_flag.set(true);
        let session = inference_session.clone();
        let input_name = inference_input_name.clone();
        let output_names = inference_output_names.clone();
        let client = inference_client.clone();
        let last_summary = inference_last_summary.clone();
        let pending_flag = inference_pending_flag.clone();
        let last_has_detection = inference_last_has_detection.clone();
        let count_ref = inference_count_ref.clone();

        spawn_local(async move {
            let outcome = infer_once(&session, &input_name, &output_names, &client, &last_has_detection).await;

            match outcome {
                Ok(summary) => {
                    let count = count_ref.get() + 1;
                    count_ref.set(count);

                    update_face_status(&input_name, &output_names, &summary);
                    *last_summary.borrow_mut() = Some(summary);

                    if count >= 20 {
                        let _ = log("workflow finished automatically after 20 inferences");
                        let _ = stop();
                    }
                }
                Err(error) => {
                    let message = describe_js_error(&error);
                    face_set_status(&format!("face detection: inference error\n{message}"));
                    let _ = log(&format!("inference error: {message}"));
                }
            }

            pending_flag.set(false);
        });
    }) as Box<dyn FnMut()>);

    let render_last_summary = last_summary.clone();
    let render_closure = Closure::wrap(Box::new(move || {
        let detections = render_last_summary
            .borrow()
            .as_ref()
            .map(|summary| summary.detections.clone())
            .unwrap_or_default();
        let _ = face_render(&detections);
    }) as Box<dyn FnMut()>);

    let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
    let inference_interval_id = window.set_interval_with_callback_and_timeout_and_arguments_0(
        inference_closure.as_ref().unchecked_ref(),
        FACE_INFERENCE_INTERVAL_MS,
    )?;
    let render_interval_id = window.set_interval_with_callback_and_timeout_and_arguments_0(
        render_closure.as_ref().unchecked_ref(),
        FACE_RENDER_INTERVAL_MS,
    )?;

    let stop_callback = Closure::once_into_js(move || {
        if is_running() {
            let _ = log("workflow finished automatically after 30 seconds");
            let _ = stop();
        }
    });
    window.set_timeout_with_callback_and_timeout_and_arguments_0(stop_callback.unchecked_ref(), 30000)?;

    let startup_summary = DetectionSummary {
        detections: Vec::new(),
        confidence: 0.0,
        processed_at: String::from("waiting for first inference"),
    };
    update_face_status(&input_name, &output_names, &startup_summary);
    log("face detection demo started")?;

    FACE_RUNTIME.with(|runtime| {
        *runtime.borrow_mut() = Some(FaceDetectionRuntime {
            client,
            capture,
            inference_interval_id,
            render_interval_id,
            _inference_closure: inference_closure,
            _render_closure: render_closure,
        });
    });

    Ok(())
}

#[wasm_bindgen]
pub async fn start() -> Result<(), JsValue> {
    run().await
}

#[wasm_bindgen]
pub fn stop() -> Result<(), JsValue> {
    FACE_RUNTIME.with(|runtime| {
        let Some(mut runtime) = runtime.borrow_mut().take() else {
            return Ok(());
        };

        let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
        window.clear_interval_with_handle(runtime.inference_interval_id);
        window.clear_interval_with_handle(runtime.render_interval_id);
        runtime.capture.stop();
        runtime.client.disconnect();
        face_detach_stream();
        face_set_status("face detection demo stopped.");
        log("face detection demo stopped")?;
        Ok(())
    })
}

async fn infer_once(
    session: &JsValue,
    input_name: &str,
    output_names: &[String],
    client: &WsClient,
    last_has_detection: &Cell<bool>,
) -> Result<DetectionSummary, JsValue> {
    let capture = face_capture_input_tensor()?;
    let tensor = create_tensor(&Float32Array::from(capture.data.as_slice()))?;
    let feeds = js_sys::Object::new();
    Reflect::set(&feeds, &JsValue::from_str(input_name), &tensor)?;

    let run_value = method(session, "run")?.call1(session, &feeds)?;
    let outputs = JsFuture::from(
        run_value
            .dyn_into::<Promise>()
            .map_err(|_| JsValue::from_str("InferenceSession.run did not return a Promise"))?,
    )
    .await?;

    let summary = decode_retinaface_outputs(
        &outputs,
        output_names,
        capture.resize_ratio,
        capture.source_width,
        capture.source_height,
    )?;
    let has_detection = !summary.detections.is_empty();
    let changed = last_has_detection.get() != has_detection;
    last_has_detection.set(has_detection);

    client.send_client_event(
        "face_detection",
        "inference",
        json!({
            "mode": "detection",
            "detected_class": if has_detection { "face" } else { "no_detection" },
            "class_index": if has_detection { 0 } else { -1 },
            "confidence": summary.confidence,
            "detections": summary
                .detections
                .iter()
                .map(|entry| json!({
                    "label": entry.label,
                    "class_index": entry.class_index,
                    "score": entry.score,
                    "box": entry.box_coords,
                }))
                .collect::<Vec<_>>(),
            "changed": changed,
            "processed_at": summary.processed_at,
            "model_path": FACE_MODEL_PATH,
            "input_name": input_name,
            "output_names": output_names,
            "source_resolution": {
                "width": capture.source_width,
                "height": capture.source_height,
            },
        }),
    )?;

    Ok(summary)
}

fn update_face_status(input_name: &str, output_names: &[String], summary: &DetectionSummary) {
    let mut lines = vec![
        String::from("face detection demo"),
        format!("model file: {FACE_MODEL_PATH}"),
        format!("input: {input_name}"),
        format!("outputs: {}", output_names.join(", ")),
        format!("detections: {}", summary.detections.len()),
        format!("best confidence: {:.4}", summary.confidence),
        format!("processed at: {}", summary.processed_at),
    ];

    if let Some(best) = summary.detections.first() {
        lines.push(String::new());
        lines.push(format!(
            "best box: {:.1}, {:.1}, {:.1}, {:.1}",
            best.box_coords[0], best.box_coords[1], best.box_coords[2], best.box_coords[3]
        ));
    }

    face_set_status(&lines.join("\n"));
}

fn decode_retinaface_outputs(
    outputs: &JsValue,
    output_names: &[String],
    resize_ratio: f64,
    source_width: f64,
    source_height: f64,
) -> Result<DetectionSummary, JsValue> {
    let loc_tensor = Reflect::get(outputs, &JsValue::from_str(&output_names[0]))?;
    let conf_tensor = Reflect::get(outputs, &JsValue::from_str(&output_names[1]))?;
    let landm_tensor = Reflect::get(outputs, &JsValue::from_str(&output_names[2]))?;

    let loc_values = tensor_f32_values(&loc_tensor)?;
    let conf_values = tensor_f32_values(&conf_tensor)?;
    let landm_values = tensor_f32_values(&landm_tensor)?;
    let prior_count = loc_values.len() / 4;
    if prior_count == 0 || conf_values.len() / 2 != prior_count || landm_values.len() / 10 != prior_count {
        return Err(JsValue::from_str("RetinaFace outputs had unexpected shapes"));
    }

    let priors = build_retinaface_priors(FACE_INPUT_HEIGHT_F64, FACE_INPUT_WIDTH_F64);
    if priors.len() != prior_count {
        return Err(JsValue::from_str("RetinaFace priors did not match output count"));
    }

    let mut detections = Vec::new();
    for index in 0..prior_count {
        let score = softmax(&[f64::from(conf_values[index * 2]), f64::from(conf_values[index * 2 + 1])])[1];
        if score < RETINAFACE_CONFIDENCE_THRESHOLD {
            continue;
        }

        let decoded = decode_retinaface_box(
            [
                f64::from(loc_values[index * 4]),
                f64::from(loc_values[index * 4 + 1]),
                f64::from(loc_values[index * 4 + 2]),
                f64::from(loc_values[index * 4 + 3]),
            ],
            priors[index],
        );
        let box_coords = [
            clamp((decoded[0] * FACE_INPUT_WIDTH_F64) / resize_ratio, 0.0, source_width),
            clamp((decoded[1] * FACE_INPUT_HEIGHT_F64) / resize_ratio, 0.0, source_height),
            clamp((decoded[2] * FACE_INPUT_WIDTH_F64) / resize_ratio, 0.0, source_width),
            clamp((decoded[3] * FACE_INPUT_HEIGHT_F64) / resize_ratio, 0.0, source_height),
        ];

        detections.push(Detection {
            label: String::from("face"),
            class_index: 0,
            score,
            box_coords,
        });
    }

    let detections = apply_nms(detections, RETINAFACE_NMS_THRESHOLD);
    let confidence = detections.first().map(|entry| entry.score).unwrap_or(0.0);
    Ok(DetectionSummary {
        detections,
        confidence,
        processed_at: String::from(js_sys::Date::new_0().to_locale_time_string("en-US")),
    })
}

fn tensor_f32_values(tensor: &JsValue) -> Result<Vec<f32>, JsValue> {
    let data = Reflect::get(tensor, &JsValue::from_str("data"))?;
    Ok(Float32Array::new(&data).to_vec())
}

fn build_retinaface_priors(image_height: f64, image_width: f64) -> Vec<[f64; 4]> {
    let mut priors = Vec::new();

    for (index, step) in RETINAFACE_STEPS.into_iter().enumerate() {
        let feature_map_height = (image_height / step).ceil() as usize;
        let feature_map_width = (image_width / step).ceil() as usize;
        let min_sizes = RETINAFACE_MIN_SIZES[index];

        for row in 0..feature_map_height {
            for column in 0..feature_map_width {
                for min_size in min_sizes {
                    priors.push([
                        (((column as f64) + 0.5) * step) / image_width,
                        (((row as f64) + 0.5) * step) / image_height,
                        min_size / image_width,
                        min_size / image_height,
                    ]);
                }
            }
        }
    }

    priors
}

fn decode_retinaface_box(loc: [f64; 4], prior: [f64; 4]) -> [f64; 4] {
    let center_x = prior[0] + loc[0] * RETINAFACE_VARIANCES[0] * prior[2];
    let center_y = prior[1] + loc[1] * RETINAFACE_VARIANCES[0] * prior[3];
    let width = prior[2] * (loc[2] * RETINAFACE_VARIANCES[1]).exp();
    let height = prior[3] * (loc[3] * RETINAFACE_VARIANCES[1]).exp();

    [
        center_x - width / 2.0,
        center_y - height / 2.0,
        center_x + width / 2.0,
        center_y + height / 2.0,
    ]
}

fn compute_iou(left: &Detection, right: &Detection) -> f64 {
    let x1 = left.box_coords[0].max(right.box_coords[0]);
    let y1 = left.box_coords[1].max(right.box_coords[1]);
    let x2 = left.box_coords[2].min(right.box_coords[2]);
    let y2 = left.box_coords[3].min(right.box_coords[3]);
    let width = (x2 - x1 + 1.0).max(0.0);
    let height = (y2 - y1 + 1.0).max(0.0);
    let intersection = width * height;
    let left_area = (left.box_coords[2] - left.box_coords[0] + 1.0).max(0.0)
        * (left.box_coords[3] - left.box_coords[1] + 1.0).max(0.0);
    let right_area = (right.box_coords[2] - right.box_coords[0] + 1.0).max(0.0)
        * (right.box_coords[3] - right.box_coords[1] + 1.0).max(0.0);

    intersection / (left_area + right_area - intersection).max(1e-6)
}

fn apply_nms(mut detections: Vec<Detection>, threshold: f64) -> Vec<Detection> {
    detections.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut kept = Vec::new();
    'candidates: for candidate in detections {
        for accepted in &kept {
            if compute_iou(&candidate, accepted) > threshold {
                continue 'candidates;
            }
        }
        kept.push(candidate);
    }

    kept
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

fn clamp(value: f64, min: f64, max: f64) -> f64 {
    value.max(min).min(max)
}

fn method(target: &JsValue, name: &str) -> Result<Function, JsValue> {
    Reflect::get(target, &JsValue::from_str(name))?
        .dyn_into::<Function>()
        .map_err(|_| JsValue::from_str(&format!("{name} is not callable")))
}

fn first_string_entry(target: &JsValue, field: &str) -> Result<String, JsValue> {
    let values = Reflect::get(target, &JsValue::from_str(field))?;
    let first = Reflect::get(&values, &JsValue::from_f64(0.0))?;
    first
        .as_string()
        .ok_or_else(|| JsValue::from_str(&format!("Missing first entry for {field}")))
}

fn string_entries(target: &JsValue, field: &str) -> Result<Vec<String>, JsValue> {
    let values = Reflect::get(target, &JsValue::from_str(field))?;
    let array = Array::from(&values);
    let mut entries = Vec::with_capacity(array.length() as usize);

    for value in array.iter() {
        let entry = value
            .as_string()
            .ok_or_else(|| JsValue::from_str(&format!("Invalid entry in {field}")))?;
        entries.push(entry);
    }

    Ok(entries)
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

async fn create_face_session(model_path: &str) -> Result<JsValue, JsValue> {
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

fn create_tensor(values: &Float32Array) -> Result<JsValue, JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
    let ort = Reflect::get(window.as_ref(), &JsValue::from_str("ort"))?;
    let tensor_ctor = Reflect::get(&ort, &JsValue::from_str("Tensor"))?
        .dyn_into::<Function>()
        .map_err(|_| JsValue::from_str("ort.Tensor is not callable"))?;

    let dims = Array::new();
    dims.push(&JsValue::from_f64(1.0));
    dims.push(&JsValue::from_f64(FACE_INPUT_HEIGHT_F64));
    dims.push(&JsValue::from_f64(FACE_INPUT_WIDTH_F64));
    dims.push(&JsValue::from_f64(3.0));

    let args = Array::new();
    args.push(&JsValue::from_str("float32"));
    args.push(values);
    args.push(&dims.into());

    Reflect::construct(&tensor_ctor, &args)
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

fn describe_js_error(error: &JsValue) -> String {
    error
        .as_string()
        .or_else(|| js_sys::JSON::stringify(error).ok().map(String::from))
        .unwrap_or_else(|| format!("{error:?}"))
}

fn log(message: &str) -> Result<(), JsValue> {
    let line = format!("[face-detection] {message}");
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

async fn face_attach_stream(stream: JsValue) -> Result<(), JsValue> {
    let video = face_video_element()?;
    let stream = stream
        .dyn_into::<MediaStream>()
        .map_err(|_| JsValue::from_str("Video capture stream was not a MediaStream"))?;

    Reflect::set(video.as_ref(), &JsValue::from_str("srcObject"), stream.as_ref())?;
    set_hidden(video.as_ref(), false)?;

    for _ in 0..50 {
        if video.video_width() > 0 && video.video_height() > 0 {
            break;
        }
        sleep_ms(100).await?;
    }

    if video.video_width() == 0 || video.video_height() == 0 {
        return Err(JsValue::from_str("Video stream metadata did not load"));
    }

    if let Ok(play_result) = method(video.as_ref(), "play").and_then(|play| play.call0(video.as_ref()))
        && let Ok(play_promise) = play_result.dyn_into::<Promise>()
    {
        let _ = JsFuture::from(play_promise).await;
    }

    Ok(())
}

fn face_detach_stream() {
    if let Ok(video) = face_video_element() {
        if let Ok(pause) = method(video.as_ref(), "pause") {
            let _ = pause.call0(video.as_ref());
        }
        let _ = Reflect::set(video.as_ref(), &JsValue::from_str("srcObject"), &JsValue::NULL);
        let _ = set_hidden(video.as_ref(), true);
    }

    if let Ok(canvas) = face_output_canvas_element() {
        let _ = set_hidden(canvas.as_ref(), true);
        if let Ok(context) = canvas_2d_context(&canvas) {
            context.clear_rect(0.0, 0.0, f64::from(canvas.width()), f64::from(canvas.height()));
        }
    }
}

fn face_set_status(message: &str) {
    let _ = set_textarea_value("module-output", message);
}

fn face_capture_input_tensor() -> Result<FaceCaptureTensor, JsValue> {
    let video = face_video_element()?;
    let source_width = f64::from(video.video_width());
    let source_height = f64::from(video.video_height());
    if source_width <= 0.0 || source_height <= 0.0 {
        return Err(JsValue::from_str("Video stream is not ready yet."));
    }

    let canvas = face_preprocess_canvas()?;
    canvas.set_width(FACE_INPUT_WIDTH as u32);
    canvas.set_height(FACE_INPUT_HEIGHT as u32);
    let context = canvas_2d_context(&canvas)?;

    let target_ratio = FACE_INPUT_HEIGHT_F64 / FACE_INPUT_WIDTH_F64;
    let resize_ratio = if source_height / source_width <= target_ratio {
        FACE_INPUT_WIDTH_F64 / source_width
    } else {
        FACE_INPUT_HEIGHT_F64 / source_height
    };

    let resized_width = (source_width * resize_ratio).round().clamp(1.0, FACE_INPUT_WIDTH_F64);
    let resized_height = (source_height * resize_ratio).round().clamp(1.0, FACE_INPUT_HEIGHT_F64);
    context.clear_rect(0.0, 0.0, FACE_INPUT_WIDTH_F64, FACE_INPUT_HEIGHT_F64);
    context.draw_image_with_html_video_element_and_dw_and_dh(&video, 0.0, 0.0, resized_width, resized_height)?;

    let image_data = context.get_image_data(0.0, 0.0, FACE_INPUT_WIDTH_F64, FACE_INPUT_HEIGHT_F64)?;
    let tensor_data = image_data_to_tensor(&image_data);

    Ok(FaceCaptureTensor {
        data: tensor_data,
        resize_ratio,
        source_width,
        source_height,
    })
}

fn face_render(detections: &[Detection]) -> Result<(), JsValue> {
    let video = face_video_element()?;
    let width = video.video_width();
    let height = video.video_height();
    if width == 0 || height == 0 {
        return Ok(());
    }

    let canvas = face_output_canvas_element()?;
    let context = canvas_2d_context(&canvas)?;
    if canvas.width() != width || canvas.height() != height {
        canvas.set_width(width);
        canvas.set_height(height);
    }

    set_hidden(canvas.as_ref(), false)?;
    context.draw_image_with_html_video_element_and_dw_and_dh(&video, 0.0, 0.0, f64::from(width), f64::from(height))?;
    context.set_line_width(3.0);
    context.set_font("16px ui-monospace, monospace");

    for detection in detections {
        let left = detection.box_coords[0];
        let top = detection.box_coords[1];
        let right = detection.box_coords[2];
        let bottom = detection.box_coords[3];
        let box_width = (right - left).max(1.0);
        let box_height = (bottom - top).max(1.0);

        context.set_stroke_style_str("#ef8f35");
        context.stroke_rect(left, top, box_width, box_height);

        let label = format!("{} {:.1}%", detection.label, detection.score * 100.0);
        let text_width = context.measure_text(&label)?.width() + 10.0;
        context.set_fill_style_str("#182028");
        context.fill_rect(left, (top - 24.0).max(0.0), text_width, 22.0);
        context.set_fill_style_str("#fffdfa");
        context.fill_text(&label, left + 5.0, (top - 8.0).max(16.0))?;
    }

    Ok(())
}

fn image_data_to_tensor(image_data: &ImageData) -> Vec<f32> {
    const CHANNEL_MEAN: [f32; 3] = [104.0, 117.0, 123.0];

    let rgba = image_data.data().to_vec();
    let mut tensor_data = vec![0.0_f32; FACE_INPUT_WIDTH * FACE_INPUT_HEIGHT * 3];

    for pixel_index in 0..(FACE_INPUT_WIDTH * FACE_INPUT_HEIGHT) {
        let rgba_index = pixel_index * 4;
        let tensor_index = pixel_index * 3;
        let red = rgba[rgba_index] as f32;
        let green = rgba[rgba_index + 1] as f32;
        let blue = rgba[rgba_index + 2] as f32;

        tensor_data[tensor_index] = blue - CHANNEL_MEAN[0];
        tensor_data[tensor_index + 1] = green - CHANNEL_MEAN[1];
        tensor_data[tensor_index + 2] = red - CHANNEL_MEAN[2];
    }

    tensor_data
}

fn face_video_element() -> Result<HtmlVideoElement, JsValue> {
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| JsValue::from_str("No document available"))?;
    document
        .get_element_by_id("video-preview")
        .ok_or_else(|| JsValue::from_str("Missing #video-preview element"))?
        .dyn_into::<HtmlVideoElement>()
        .map_err(|_| JsValue::from_str("#video-preview was not a video element"))
}

fn face_output_canvas_element() -> Result<HtmlCanvasElement, JsValue> {
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| JsValue::from_str("No document available"))?;
    document
        .get_element_by_id("video-output-canvas")
        .ok_or_else(|| JsValue::from_str("Missing #video-output-canvas element"))?
        .dyn_into::<HtmlCanvasElement>()
        .map_err(|_| JsValue::from_str("#video-output-canvas was not a canvas element"))
}

fn face_preprocess_canvas() -> Result<HtmlCanvasElement, JsValue> {
    FACE_PREPROCESS_CANVAS.with(|slot| {
        if let Some(canvas) = slot.borrow().as_ref() {
            return Ok(canvas.clone());
        }

        let document = web_sys::window()
            .and_then(|window| window.document())
            .ok_or_else(|| JsValue::from_str("No document available"))?;
        let canvas = document
            .create_element("canvas")?
            .dyn_into::<HtmlCanvasElement>()
            .map_err(|_| JsValue::from_str("Unable to create preprocessing canvas"))?;
        *slot.borrow_mut() = Some(canvas.clone());
        Ok(canvas)
    })
}

fn canvas_2d_context(canvas: &HtmlCanvasElement) -> Result<CanvasRenderingContext2d, JsValue> {
    canvas
        .get_context("2d")?
        .ok_or_else(|| JsValue::from_str("2d canvas context was unavailable"))?
        .dyn_into::<CanvasRenderingContext2d>()
        .map_err(|_| JsValue::from_str("Canvas context was not 2d"))
}

fn set_hidden(target: &JsValue, hidden: bool) -> Result<(), JsValue> {
    Reflect::set(target, &JsValue::from_str("hidden"), &JsValue::from_bool(hidden)).map(|_| ())
}

#[cfg(test)]
mod test_face_detection;
