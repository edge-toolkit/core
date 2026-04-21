use std::cell::{Cell, RefCell};
use std::rc::Rc;

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
pub struct SpeechRecognitionSession {
    recognition: JsValue,
    stop_requested: Rc<Cell<bool>>,
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

struct SpeechRecognitionRuntime {
    client: WsClient,
    session: Rc<SpeechRecognitionSession>,
}

thread_local! {
    static SPEECH_RECOGNITION_RUNTIME: RefCell<Option<SpeechRecognitionRuntime>> = const { RefCell::new(None) };
}

#[wasm_bindgen(start)]
pub fn init() {
    let _ = tracing_wasm::try_set_as_global_default();
    info!("speech-recognition module initialized");
}

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
    log("entered run()")?;

    let ws_url = websocket_url()?;
    let mut client = WsClient::new(WsClientConfig::new(ws_url));
    client.connect()?;
    wait_for_connected(&client).await?;
    log(&format!("websocket connected with agent_id={}", client.get_client_id()))?;

    log("starting speech recognition session")?;
    let session = Rc::new(SpeechRecognitionSession::new()?);

    SPEECH_RECOGNITION_RUNTIME.with(|runtime| {
        runtime.borrow_mut().replace(SpeechRecognitionRuntime {
            client: client.clone(),
            session: session.clone(),
        });
    });

    set_module_status("speech-recognition: running")?;

    let start_time = js_sys::Date::now();
    let mut result_count = 0;

    while is_running() {
        let elapsed_ms = js_sys::Date::now() - start_time;
        if elapsed_ms > 30000.0 {
            let _ = log("workflow finished automatically after 30 seconds");
            let _ = stop();
            break;
        }
        if result_count >= 3 {
            let _ = log("workflow finished automatically after 3 recognition results");
            let _ = stop();
            break;
        }

        log("awaiting speech recognition...")?;
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
                    "speech recognized: \"{}\" (confidence={})",
                    transcript, confidence
                ))?;

                client.send_client_event(
                    "speech",
                    "recognition_result",
                    json!({
                        "transcript": transcript,
                        "confidence": confidence,
                    }),
                )?;

                set_module_status(&format!(
                    "speech-recognition: result\n\"{}\"\nconfidence: {}",
                    transcript, confidence
                ))?;
            }
            Err(error) => {
                let message = describe_js_error(&error);
                log(&format!("recognition error: {}", message))?;
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
            let _ = runtime.session.stop();
            runtime.client.disconnect();
            log("speech-recognition stopped")?;
        }
        Ok::<(), JsValue>(())
    })?;

    set_module_status("speech-recognition: stopped")?;
    Ok(())
}

fn log(message: &str) -> Result<(), JsValue> {
    let line = format!("[speech-recognition] {}", message);
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
