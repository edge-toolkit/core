#![expect(
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::float_arithmetic,
    clippy::future_not_send,
    clippy::single_call_fn,
    reason = "browser WASM module: JsFuture is !Send; module-local helpers and confidence averaging math are inherent"
)]

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use et_web::JsFunctionExt as _;
use et_ws_wasm_agent::{WsClient, WsClientConfig, set_textarea_value};
use js_sys::{Promise, Reflect};
use serde_json::json;
use tracing::info;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen]
pub struct SpeechRecognitionResult {
    transcript: String,
    confidence: f64,
}

#[wasm_bindgen]
#[expect(clippy::missing_const_for_fn, reason = "wasm_bindgen rejects const fns")]
impl SpeechRecognitionResult {
    #[wasm_bindgen(js_name = recognizeOnce)]
    pub async fn recognize_once() -> Result<Self, JsValue> {
        let session = SpeechRecognitionSession::new()?;
        session.start().await
    }

    #[must_use]
    pub fn transcript(&self) -> String {
        self.transcript.clone()
    }

    #[must_use]
    pub fn confidence(&self) -> f64 {
        self.confidence
    }
}

#[wasm_bindgen]
pub struct SpeechRecognitionSession {
    recognition: JsValue,
    stop_requested: Rc<Cell<bool>>,
}

#[wasm_bindgen]
impl SpeechRecognitionSession {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Result<Self, JsValue> {
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
        let constructor = speech_recognition_ctor.into_function("SpeechRecognition constructor")?;
        let recognition = js_sys::Reflect::construct(&constructor, &js_sys::Array::new())?;

        let _: bool = js_sys::Reflect::set(&recognition, &JsValue::from_str("lang"), &JsValue::from_str("en-US"))?;
        let _: bool = js_sys::Reflect::set(&recognition, &JsValue::from_str("interimResults"), &JsValue::TRUE)?;
        let _: bool = js_sys::Reflect::set(
            &recognition,
            &JsValue::from_str("maxAlternatives"),
            &JsValue::from_f64(1.0),
        )?;

        Ok(Self {
            recognition,
            stop_requested: Rc::new(Cell::new(false)),
        })
    }

    #[expect(
        clippy::too_many_lines,
        reason = "single-method wiring of Web Speech API onresult/onerror/onend handlers and the wrapping promise"
    )]
    pub async fn start(&self) -> Result<SpeechRecognitionResult, JsValue> {
        self.stop_requested.set(false);
        let recognition = self.recognition.clone();
        let stop_requested = Rc::clone(&self.stop_requested);
        let promise = js_sys::Promise::new(&mut |resolve, reject| {
            let settled = Rc::new(Cell::new(false));
            let resolve_for_result = resolve.clone();
            let resolve_for_end = resolve;
            let reject_for_error = reject.clone();
            let reject_for_end = reject.clone();
            let settled_for_result = Rc::clone(&settled);
            let settled_for_error = Rc::clone(&settled);
            let settled_for_end = Rc::clone(&settled);
            let transcript_state: Rc<RefCell<Option<(String, f64)>>> = Rc::new(RefCell::new(None));
            let transcript_state_for_result = Rc::clone(&transcript_state);
            let transcript_state_for_end = Rc::clone(&transcript_state);
            let stop_requested_for_end = Rc::clone(&stop_requested);

            let on_result_box: Box<dyn FnMut(JsValue)> = Box::new(move |event: JsValue| {
                if let Some((transcript, confidence, has_final)) = extract_speech_event_transcript(&event) {
                    *transcript_state_for_result.borrow_mut() = Some((transcript.clone(), confidence));

                    if has_final && !settled_for_result.replace(true) {
                        let payload = js_sys::Object::new();
                        et_web::ignore(js_sys::Reflect::set(
                            &payload,
                            &JsValue::from_str("transcript"),
                            &JsValue::from_str(&transcript),
                        ));
                        et_web::ignore(js_sys::Reflect::set(
                            &payload,
                            &JsValue::from_str("confidence"),
                            &JsValue::from_f64(confidence),
                        ));
                        et_web::ignore(resolve_for_result.call1(&JsValue::NULL, &payload));
                    }
                }
            });
            let on_result = Closure::wrap(on_result_box);

            let on_error_box: Box<dyn FnMut(JsValue)> = Box::new(move |event: JsValue| {
                if settled_for_error.replace(true) {
                    return;
                }
                let message = js_sys::Reflect::get(&event, &JsValue::from_str("error"))
                    .ok()
                    .and_then(|value| value.as_string())
                    .unwrap_or_else(|| "speech recognition failed".to_string());
                et_web::ignore(reject_for_error.call1(&JsValue::NULL, &JsValue::from_str(&message)));
            });
            let on_error = Closure::wrap(on_error_box);

            let on_end_box: Box<dyn FnMut()> = Box::new(move || {
                if settled_for_end.replace(true) {
                    return;
                }
                if let Some((transcript, confidence)) = transcript_state_for_end.borrow().clone() {
                    let payload = js_sys::Object::new();
                    et_web::ignore(js_sys::Reflect::set(
                        &payload,
                        &JsValue::from_str("transcript"),
                        &JsValue::from_str(&transcript),
                    ));
                    et_web::ignore(js_sys::Reflect::set(
                        &payload,
                        &JsValue::from_str("confidence"),
                        &JsValue::from_f64(confidence),
                    ));
                    et_web::ignore(resolve_for_end.call1(&JsValue::NULL, &payload));
                } else if stop_requested_for_end.get() {
                    et_web::ignore(reject_for_end.call1(
                        &JsValue::NULL,
                        &JsValue::from_str("speech recognition stopped before any transcript was captured"),
                    ));
                } else {
                    et_web::ignore(reject_for_end.call1(
                        &JsValue::NULL,
                        &JsValue::from_str("speech recognition ended without a transcript"),
                    ));
                }
            });
            let on_end = Closure::wrap(on_end_box);

            et_web::ignore(js_sys::Reflect::set(
                &recognition,
                &JsValue::from_str("onresult"),
                on_result.as_ref().unchecked_ref(),
            ));
            et_web::ignore(js_sys::Reflect::set(
                &recognition,
                &JsValue::from_str("onerror"),
                on_error.as_ref().unchecked_ref(),
            ));
            et_web::ignore(js_sys::Reflect::set(
                &recognition,
                &JsValue::from_str("onend"),
                on_end.as_ref().unchecked_ref(),
            ));

            match js_sys::Reflect::get(&recognition, &JsValue::from_str("start"))
                .and_then(|value| value.into_function("SpeechRecognition.start"))
            {
                Ok(start) => {
                    et_web::ignore(start.call0(&recognition));
                }
                Err(err) => {
                    et_web::ignore(reject.call1(&JsValue::NULL, &err));
                }
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
            .unwrap_or(0.0_f64);

        info!("Speech recognition captured transcript with confidence={}", confidence);

        Ok(SpeechRecognitionResult { transcript, confidence })
    }

    pub fn stop(&self) -> Result<(), JsValue> {
        self.stop_requested.set(true);
        let stop = js_sys::Reflect::get(&self.recognition, &JsValue::from_str("stop"))?
            .into_function("SpeechRecognition.stop")?;
        let _v: JsValue = stop.call0(&self.recognition)?;
        Ok(())
    }
}

fn extract_speech_event_transcript(event: &JsValue) -> Option<(String, f64, bool)> {
    let results = js_sys::Reflect::get(event, &JsValue::from_str("results")).ok()?;
    let length = js_sys::Reflect::get(&results, &JsValue::from_str("length"))
        .ok()?
        .as_f64()? as u32;

    let mut transcript_parts = Vec::new();
    let mut confidence = 0.0_f64;
    let mut confidence_count = 0_u32;
    let mut has_final = false;

    for index in 0..length {
        let Ok(result) = js_sys::Reflect::get(&results, &JsValue::from_f64(f64::from(index))) else {
            continue;
        };

        let Ok(alternative) = js_sys::Reflect::get(&result, &JsValue::from_f64(0.0)) else {
            continue;
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
        0.0_f64
    } else {
        confidence / f64::from(confidence_count)
    };

    Some((transcript, average_confidence, has_final))
}

struct SpeechRecognitionRuntime {
    client: WsClient,
    session: Rc<SpeechRecognitionSession>,
}

thread_local! {
    static SPEECH_RECOGNITION_RUNTIME: RefCell<Option<SpeechRecognitionRuntime>> = const { RefCell::new(None) };
}

#[wasm_bindgen(start)]
pub fn init() {
    et_web::ignore(tracing_wasm::try_set_as_global_default());
    info!("speech-recognition module initialized");
}

#[must_use]
#[wasm_bindgen]
pub fn is_running() -> bool {
    SPEECH_RECOGNITION_RUNTIME.with(|runtime| runtime.borrow().is_some())
}

#[wasm_bindgen]
pub async fn run() -> Result<(), JsValue> {
    if is_running() {
        return Ok(());
    }

    set_module_status("speech-recognition: entered run()")?;
    log("entered run()");

    let ws_url = websocket_url()?;
    let mut client = WsClient::new(WsClientConfig::new(ws_url));
    client.connect()?;
    wait_for_connected(&client).await?;
    log(&format!("websocket connected with agent_id={}", client.get_agent_id()));

    log("starting speech recognition session");
    let session = Rc::new(SpeechRecognitionSession::new()?);

    SPEECH_RECOGNITION_RUNTIME.with(|runtime| {
        let _previous: Option<SpeechRecognitionRuntime> = runtime.borrow_mut().replace(SpeechRecognitionRuntime {
            client: client.clone(),
            session: Rc::clone(&session),
        });
    });

    set_module_status("speech-recognition: running")?;

    let start_time = js_sys::Date::now();
    let mut result_count = 0_u32;

    while is_running() {
        let elapsed_ms = js_sys::Date::now() - start_time;
        if elapsed_ms > 30_000_f64 {
            log("workflow finished automatically after 30 seconds");
            et_web::ignore(stop());
            break;
        }
        if result_count >= 3 {
            log("workflow finished automatically after 3 recognition results");
            et_web::ignore(stop());
            break;
        }

        log("awaiting speech recognition...");
        let result_outcome = session.start().await;

        if !is_running() {
            break;
        }

        match result_outcome {
            Ok(result) => {
                result_count += 1;
                let transcript = result.transcript();
                let confidence = result.confidence();
                log(&format!(
                    "speech recognized: \"{transcript}\" (confidence={confidence})"
                ));

                client.send_client_event(
                    "speech",
                    "recognition_result",
                    json!({
                        "transcript": transcript,
                        "confidence": confidence,
                    }),
                )?;

                set_module_status(&format!(
                    "speech-recognition: result\n\"{transcript}\"\nconfidence: {confidence}"
                ))?;
            }
            Err(error) => {
                let message = describe_js_error(&error);
                log(&format!("recognition error: {message}"));
                // Sleep a bit before retrying to avoid tight error loops
                sleep_ms(1000).await?;
            }
        }
    }

    Ok(())
}

#[wasm_bindgen]
pub fn stop() -> Result<(), JsValue> {
    SPEECH_RECOGNITION_RUNTIME.with(|runtime| {
        if let Some(mut runtime) = runtime.borrow_mut().take() {
            et_web::ignore(runtime.session.stop());
            runtime.client.disconnect();
            log("speech-recognition stopped");
        }
    });

    set_module_status("speech-recognition: stopped")?;
    Ok(())
}

fn log(message: &str) {
    let line = format!("[speech-recognition] {message}");
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
}

fn set_module_status(message: &str) -> Result<(), JsValue> {
    set_textarea_value("module-output", message)
}

fn describe_js_error(error: &JsValue) -> String {
    error
        .as_string()
        .or_else(|| js_sys::JSON::stringify(error).ok().map(String::from))
        .unwrap_or_else(|| format!("{error:?}"))
}

async fn wait_for_connected(client: &WsClient) -> Result<(), JsValue> {
    for _ in 0_u32..100 {
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
            et_web::ignore(resolve.call0(&JsValue::NULL));
        });

        if let Err(error) =
            window.set_timeout_with_callback_and_timeout_and_arguments_0(callback.unchecked_ref(), duration_ms)
        {
            et_web::ignore(reject.call1(&JsValue::NULL, &error));
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
    Ok(format!("{ws_protocol}//{host}/ws"))
}
