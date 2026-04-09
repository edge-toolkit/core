use std::cell::{Cell, RefCell};
use std::rc::Rc;

use et_ws_wasm_agent::{VideoCapture, WsClient, WsClientConfig};
use js_sys::{Array, Float32Array, Function, Promise, Reflect};
use serde_json::json;
use tracing::info;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{JsFuture, spawn_local};

const FACE_MODEL_PATH: &str = "/static/models/video_cv.onnx";
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

#[wasm_bindgen(inline_js = r##"
export async function face_attach_stream(stream) {
  const video = document.getElementById("face-video-preview");
  if (!video) {
    throw new Error("Missing #face-video-preview element");
  }

  video.srcObject = stream;
  video.hidden = false;

  if (!video.videoWidth || !video.videoHeight) {
    await new Promise((resolve, reject) => {
      const onLoaded = () => {
        cleanup();
        resolve();
      };
      const onError = () => {
        cleanup();
        reject(new Error("Video stream metadata did not load"));
      };
      const cleanup = () => {
        video.removeEventListener("loadedmetadata", onLoaded);
        video.removeEventListener("error", onError);
      };
      video.addEventListener("loadedmetadata", onLoaded, { once: true });
      video.addEventListener("error", onError, { once: true });
    });
  }

  const playResult = video.play?.();
  if (playResult?.catch) {
    try {
      await playResult;
    } catch {
      // Browsers may reject autoplay even after a gesture; metadata is enough for capture.
    }
  }
}

export function face_detach_stream() {
  const video = document.getElementById("face-video-preview");
  const canvas = document.getElementById("face-video-output-canvas");
  if (video) {
    video.pause?.();
    video.srcObject = null;
    video.hidden = true;
  }
  if (canvas) {
    canvas.hidden = true;
    const context = canvas.getContext("2d");
    context?.clearRect(0, 0, canvas.width, canvas.height);
  }
}

export function face_set_status(message) {
  const output = document.getElementById("face-output");
  if (output) {
    output.value = String(message);
  }
}

export function face_log(message) {
  const line = `[face-detection] ${message}`;
  console.log(line);
  const logEl = document.getElementById("log");
  if (!logEl) {
    return;
  }
  const current = logEl.textContent ?? "";
  logEl.textContent = current ? `${current}\n${line}` : line;
}

export function face_capture_input_tensor() {
  const video = document.getElementById("face-video-preview");
  if (!video?.videoWidth || !video?.videoHeight) {
    throw new Error("Video stream is not ready yet.");
  }

  const width = 640;
  const height = 608;
  const mean = [104, 117, 123];
  const canvas = globalThis.__etFacePreprocessCanvas ?? document.createElement("canvas");
  globalThis.__etFacePreprocessCanvas = canvas;
  const context = canvas.getContext("2d", { willReadFrequently: true });
  if (!context) {
    throw new Error("Unable to create face preprocessing canvas context.");
  }

  canvas.width = width;
  canvas.height = height;

  const sourceWidth = video.videoWidth;
  const sourceHeight = video.videoHeight;
  const targetRatio = height / width;
  let resizeRatio;
  if (sourceHeight / sourceWidth <= targetRatio) {
    resizeRatio = width / sourceWidth;
  } else {
    resizeRatio = height / sourceHeight;
  }

  const resizedWidth = Math.max(1, Math.min(width, Math.round(sourceWidth * resizeRatio)));
  const resizedHeight = Math.max(1, Math.min(height, Math.round(sourceHeight * resizeRatio)));
  context.clearRect(0, 0, width, height);
  context.drawImage(video, 0, 0, resizedWidth, resizedHeight);

  const rgba = context.getImageData(0, 0, width, height).data;
  const tensorData = new Float32Array(width * height * 3);

  for (let pixelIndex = 0; pixelIndex < width * height; pixelIndex += 1) {
    const rgbaIndex = pixelIndex * 4;
    const red = rgba[rgbaIndex];
    const green = rgba[rgbaIndex + 1];
    const blue = rgba[rgbaIndex + 2];
    const tensorIndex = pixelIndex * 3;
    tensorData[tensorIndex] = blue - mean[0];
    tensorData[tensorIndex + 1] = green - mean[1];
    tensorData[tensorIndex + 2] = red - mean[2];
  }

  return {
    data: tensorData,
    resizeRatio,
    sourceWidth,
    sourceHeight,
  };
}

export function face_render(detections) {
  const video = document.getElementById("face-video-preview");
  const canvas = document.getElementById("face-video-output-canvas");
  if (!video?.videoWidth || !video?.videoHeight || !canvas) {
    return;
  }

  const context = canvas.getContext("2d");
  if (!context) {
    throw new Error("Unable to create face output canvas context.");
  }

  const width = video.videoWidth;
  const height = video.videoHeight;
  if (canvas.width !== width || canvas.height !== height) {
    canvas.width = width;
    canvas.height = height;
  }

  canvas.hidden = false;
  context.drawImage(video, 0, 0, width, height);
  context.lineWidth = 3;
  context.font = "16px ui-monospace, monospace";

  for (const entry of detections ?? []) {
    const [x1, y1, x2, y2] = entry.box ?? [];
    const left = Number(x1 ?? 0);
    const top = Number(y1 ?? 0);
    const right = Number(x2 ?? 0);
    const bottom = Number(y2 ?? 0);
    const boxWidth = Math.max(1, right - left);
    const boxHeight = Math.max(1, bottom - top);
    context.strokeStyle = "#ef8f35";
    context.strokeRect(left, top, boxWidth, boxHeight);

    const label = `${entry.label ?? "face"} ${((entry.score ?? 0) * 100).toFixed(1)}%`;
    const textWidth = context.measureText(label).width + 10;
    context.fillStyle = "#182028";
    context.fillRect(left, Math.max(0, top - 24), textWidth, 22);
    context.fillStyle = "#fffdfa";
    context.fillText(label, left + 5, Math.max(16, top - 8));
  }
}
"##)]
extern "C" {
    #[wasm_bindgen(catch)]
    async fn face_attach_stream(stream: JsValue) -> Result<JsValue, JsValue>;
    #[wasm_bindgen]
    fn face_detach_stream();
    #[wasm_bindgen]
    fn face_set_status(message: &str);
    #[wasm_bindgen]
    fn face_log(message: &str);
    #[wasm_bindgen(catch)]
    fn face_capture_input_tensor() -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    fn face_render(detections: &JsValue) -> Result<(), JsValue>;
}

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

struct FaceDetectionRuntime {
    client: WsClient,
    capture: VideoCapture,
    inference_interval_id: i32,
    render_interval_id: i32,
    _inference_closure: Closure<dyn FnMut()>,
    _render_closure: Closure<dyn FnMut()>,
}

thread_local! {
    static FACE_RUNTIME: RefCell<Option<FaceDetectionRuntime>> = const { RefCell::new(None) };
}

#[wasm_bindgen(start)]
pub fn init() {
    tracing_wasm::set_as_global_default();
    info!("face detection workflow module initialized");
}

#[wasm_bindgen]
pub fn is_running() -> bool {
    FACE_RUNTIME.with(|runtime| runtime.borrow().is_some())
}

#[wasm_bindgen]
pub async fn start() -> Result<(), JsValue> {
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

    let inference_session = session.clone();
    let inference_input_name = input_name.clone();
    let inference_output_names = output_names.clone();
    let inference_client = client.clone();
    let inference_last_summary = last_summary.clone();
    let inference_pending_flag = inference_pending.clone();
    let inference_last_has_detection = last_has_detection.clone();
    let inference_closure = Closure::wrap(Box::new(move || {
        if inference_pending_flag.get() {
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

        spawn_local(async move {
            let outcome = infer_once(&session, &input_name, &output_names, &client, &last_has_detection).await;

            match outcome {
                Ok(summary) => {
                    update_face_status(&input_name, &output_names, &summary);
                    *last_summary.borrow_mut() = Some(summary);
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
            .map(|summary| detections_to_js(&summary.detections))
            .unwrap_or_else(|| Array::new().into());
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
    let tensor_data = Reflect::get(&capture, &JsValue::from_str("data"))?;
    let resize_ratio = Reflect::get(&capture, &JsValue::from_str("resizeRatio"))?
        .as_f64()
        .ok_or_else(|| JsValue::from_str("face capture resizeRatio was unavailable"))?;
    let source_width = Reflect::get(&capture, &JsValue::from_str("sourceWidth"))?
        .as_f64()
        .ok_or_else(|| JsValue::from_str("face capture sourceWidth was unavailable"))?;
    let source_height = Reflect::get(&capture, &JsValue::from_str("sourceHeight"))?
        .as_f64()
        .ok_or_else(|| JsValue::from_str("face capture sourceHeight was unavailable"))?;

    let tensor = create_tensor(&Float32Array::new(&tensor_data))?;
    let feeds = js_sys::Object::new();
    Reflect::set(&feeds, &JsValue::from_str(input_name), &tensor)?;

    let run_value = method(session, "run")?.call1(session, &feeds)?;
    let outputs = JsFuture::from(
        run_value
            .dyn_into::<Promise>()
            .map_err(|_| JsValue::from_str("InferenceSession.run did not return a Promise"))?,
    )
    .await?;

    let summary = decode_retinaface_outputs(&outputs, output_names, resize_ratio, source_width, source_height)?;
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
                "width": source_width,
                "height": source_height,
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

fn detections_to_js(detections: &[Detection]) -> JsValue {
    let array = Array::new();

    for detection in detections {
        let object = js_sys::Object::new();
        let box_values = Array::new();
        for value in detection.box_coords {
            box_values.push(&JsValue::from_f64(value));
        }
        let _ = Reflect::set(
            &object,
            &JsValue::from_str("label"),
            &JsValue::from_str(&detection.label),
        );
        let _ = Reflect::set(
            &object,
            &JsValue::from_str("score"),
            &JsValue::from_f64(detection.score),
        );
        let _ = Reflect::set(&object, &JsValue::from_str("box"), &box_values);
        array.push(&object);
    }

    array.into()
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
    face_log(message);
    Ok(())
}
