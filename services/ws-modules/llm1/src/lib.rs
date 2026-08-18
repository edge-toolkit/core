//! llm1: browser LLM chat module -- transformers.js text generation on WebGPU, entirely same-origin.
//!
//! `run()` loads the transformers.js runtime the page exposes as `window.loadTransformers`, builds a
//! text-generation pipeline over the weights the ws-server serves as `et-model-llm1`, then drives the page's
//! chat panel: every prompt the user sends is appended to the transcript, generated token-by-token through a
//! transformers.js `TextStreamer`, and reported to the server as an `et-client-event`. No request leaves the
//! ws-server origin -- runtime, ONNX weights, tokenizer and the ORT wasm binaries are all served as modules.
//!
//! The chat loop ends when the panel's close button calls `stop()`, or after `MAX_RUNTIME_POLLS` overall.

#![expect(
    clippy::future_not_send,
    clippy::single_call_fn,
    unused_results,
    reason = "browser WASM module: JsFuture is !Send; module-local helpers are single-use; Reflect::set returns bool"
)]

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use et_web::{JsFunctionExt as _, JsPromiseExt as _, JsResultExt as _};
use et_ws_wasm_agent::{WsClient, WsClientConfig, set_textarea_value};
use js_sys::{Array, Function, Object, Promise, Reflect};
use serde_json::json;
use tracing::info;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{HtmlElement, HtmlTextAreaElement, KeyboardEvent};

/// Model module holding the ONNX weights, tokenizer and configs; served at `/modules/et-model-llm1/`.
const MODEL_ID: &str = "et-model-llm1";
/// transformers.js resolves a local model as `<local_model_path>/<model_id>/`, so `/modules/` is the root.
const LOCAL_MODEL_PATH: &str = "/modules/";
/// Weight precision to load. Matches the single `onnx/model_q4f16.onnx` file the fetch task downloads.
const MODEL_DTYPE: &str = "q4f16";
/// Directory the module's build task vendors the ORT wasm runtime into, from transformers.js's own pin.
const ORT_WASM_DIR: &str = "/modules/et-ws-llm1/ort";
/// System turn prepended to every request, kept short because a 135M-parameter model follows little else.
const SYSTEM_PROMPT: &str = "You are a concise, helpful assistant running locally on an edge device.";
/// Generation cap per reply. Small enough that a CPU-fallback device still answers in a sensible time.
const MAX_NEW_TOKENS: f64 = 192.0;
/// How long the transcript may grow before the oldest turns stop being resent as context.
const MAX_HISTORY_TURNS: usize = 8;
const POLL_INTERVAL_MS: i32 = 100;
/// ~30 minutes of 100ms polls: an idle chat panel eventually closes itself rather than pending forever.
const MAX_RUNTIME_POLLS: u32 = 18_000;

thread_local! {
    /// The running chat's stop flag; `Some` while `run()` is active, shared with `stop()`.
    static STOP_FLAG: RefCell<Option<Rc<Cell<bool>>>> = const { RefCell::new(None) };
}

/// One chat turn as transformers.js wants it: a role and its content.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Turn {
    pub role: String,
    pub content: String,
}

#[wasm_bindgen(start)]
pub fn init() {
    tracing_wasm::set_as_global_default();
    info!("llm1 chat module initialized");
}

/// Return whether a chat session is currently active (the page's run button toggles run/stop on this).
#[must_use]
#[wasm_bindgen]
pub fn is_running() -> bool {
    STOP_FLAG.with(|flag| flag.borrow().is_some())
}

/// Request the active chat loop to stop; it exits at its next poll and cleans up after itself.
#[wasm_bindgen]
pub fn stop() {
    STOP_FLAG.with(|flag| {
        if let Some(stop_requested) = flag.borrow_mut().take() {
            stop_requested.set(true);
        }
    });
    log("stop requested");
}

/// Run the chat workflow: load the runtime, build the pipeline, then answer prompts until stopped.
#[wasm_bindgen]
pub async fn run() -> Result<(), JsValue> {
    if is_running() {
        return Ok(());
    }
    let stop_requested = Rc::new(Cell::new(false));
    STOP_FLAG.with(|flag| *flag.borrow_mut() = Some(Rc::clone(&stop_requested)));

    let outcome = chat_workflow(&stop_requested).await;

    STOP_FLAG.with(|flag| {
        let _active = flag.borrow_mut().take();
    });
    hide_panel();
    outcome
}

/// Connect, load the model, then serve prompts from the page's chat panel until stopped.
async fn chat_workflow(stop_requested: &Rc<Cell<bool>>) -> Result<(), JsValue> {
    log("entered run()");
    set_module_status("llm1: connecting")?;

    let mut client = WsClient::new(WsClientConfig::new(websocket_url()?));
    client.connect()?;
    wait_for_connected(&client).await?;
    let agent_id = wait_for_agent_id(&client).await?;
    log(&format!("websocket connected with agent_id={agent_id}"));

    let transformers = load_runtime().await?;
    configure_runtime(&transformers)?;
    let generator = create_generator(&transformers).await?;
    let generate = generator.clone().into_function("text-generation pipeline")?;
    let tokenizer = Reflect::get(&generator, &JsValue::from_str("tokenizer"))?;

    let prompts: Rc<RefCell<VecDeque<String>>> = Rc::new(RefCell::new(VecDeque::new()));
    let listeners = attach_panel_listeners(&prompts)?;
    show_panel()?;
    set_module_status("llm1: ready -- type a message and press Enter")?;

    let outcome = serve_prompts(
        &ChatContext {
            client: &client,
            generate: &generate,
            streamer_ctor: streamer_constructor(&transformers)?,
            tokenizer: &tokenizer,
        },
        &prompts,
        stop_requested,
    )
    .await;

    listeners.detach();
    client.disconnect();
    outcome
}

/// The per-session JS handles one chat turn needs, bundled so the turn loop takes a single argument.
struct ChatContext<'session> {
    client: &'session WsClient,
    generate: &'session Function,
    streamer_ctor: Function,
    tokenizer: &'session JsValue,
}

/// Poll the prompt queue, answering each prompt in turn, until stopped or the runtime cap is reached.
async fn serve_prompts(
    context: &ChatContext<'_>,
    prompts: &Rc<RefCell<VecDeque<String>>>,
    stop_requested: &Rc<Cell<bool>>,
) -> Result<(), JsValue> {
    let mut history: Vec<Turn> = Vec::new();
    let mut answered: u32 = 0;
    let mut total_polls: u32 = 0;

    while !stop_requested.get() {
        if total_polls >= MAX_RUNTIME_POLLS {
            log("chat closed automatically after ~30 minutes");
            break;
        }
        total_polls = total_polls.saturating_add(1);

        let next = prompts.borrow_mut().pop_front();
        let Some(prompt) = next else {
            sleep_ms(POLL_INTERVAL_MS).await?;
            continue;
        };

        append_turn("user", &prompt)?;
        history.push(Turn {
            role: "user".to_owned(),
            content: prompt.clone(),
        });
        set_module_status("llm1: generating...")?;

        let started_ms = js_sys::Date::now();
        let reply = match answer(context, &history).await {
            Ok(reply) => reply,
            Err(error) => {
                let message = format!("generation failed: {error:?}");
                log(&message);
                set_module_status(&format!("llm1: {message}"))?;
                continue;
            }
        };
        let elapsed_ms = js_sys::Date::now() - started_ms;

        history.push(Turn {
            role: "assistant".to_owned(),
            content: reply.clone(),
        });
        truncate_history(&mut history);
        answered = answered.saturating_add(1);
        set_module_status(&format!(
            "llm1\nmodel: {MODEL_ID} ({MODEL_DTYPE})\nreplies: {answered}\nlast reply: {elapsed_ms:.0} ms"
        ))?;
        context.client.send_client_event(
            "llm1",
            "replied",
            json!({
                "elapsed_ms": elapsed_ms,
                "model": MODEL_ID,
                "prompt_chars": prompt.chars().count(),
                "replies": answered,
                "reply_chars": reply.chars().count(),
            }),
        )?;
    }

    let message = format!("llm1 stopped after {answered} reply(ies).");
    log(&message);
    set_module_status(&message)?;
    Ok(())
}

/// Generate one assistant reply for the current transcript, streaming tokens into the panel as they arrive.
async fn answer(context: &ChatContext<'_>, history: &[Turn]) -> Result<String, JsValue> {
    let bubble = append_turn("assistant", "")?;
    let on_token: Box<dyn FnMut(JsValue)> = Box::new({
        let bubble = bubble.clone();
        move |token: JsValue| {
            if let Some(text) = token.as_string() {
                let current = bubble.text_content().unwrap_or_default();
                bubble.set_text_content(Some(&format!("{current}{text}")));
                scroll_log_to_end();
            }
        }
    });
    let on_token = Closure::wrap(on_token);
    let streamer = build_streamer(context, &on_token)?;

    let options = Object::new();
    Reflect::set(
        &options,
        &JsValue::from_str("max_new_tokens"),
        &JsValue::from_f64(MAX_NEW_TOKENS),
    )?;
    // Greedy decoding: deterministic replies keep the demo reproducible across runs and devices.
    Reflect::set(&options, &JsValue::from_str("do_sample"), &JsValue::FALSE)?;
    Reflect::set(&options, &JsValue::from_str("streamer"), &streamer)?;

    // Invoked through `Reflect.apply` rather than `Function::call2`: a transformers.js pipeline is a closure
    // whose prototype was replaced with its class's (their `Callable` base), so it is `typeof "function"` but
    // has no `Function.prototype.call` to reach -- calling it that way fails with
    // `TypeError: arg0.call is not a function`. Reflect.apply invokes the callable directly instead.
    let messages = chat_messages(history)?;
    let output = Reflect::apply(
        context.generate,
        &JsValue::NULL,
        &Array::of2(messages.as_ref(), &options),
    )?;
    let output = JsFuture::from(output.into_promise("text-generation pipeline")?).await?;

    let reply = extract_reply(&output)?;
    bubble.set_text_content(Some(&reply));
    scroll_log_to_end();
    Ok(reply)
}

/// Build the `TextStreamer` that forwards each decoded token to the panel while generation runs.
fn build_streamer(context: &ChatContext<'_>, on_token: &Closure<dyn FnMut(JsValue)>) -> Result<JsValue, JsValue> {
    let options = Object::new();
    Reflect::set(&options, &JsValue::from_str("skip_prompt"), &JsValue::TRUE)?;
    Reflect::set(&options, &JsValue::from_str("skip_special_tokens"), &JsValue::TRUE)?;
    Reflect::set(&options, &JsValue::from_str("callback_function"), on_token.as_ref())?;
    Reflect::construct(&context.streamer_ctor, &Array::of2(context.tokenizer, &options))
}

/// Look up the runtime's `TextStreamer` constructor.
fn streamer_constructor(transformers: &JsValue) -> Result<Function, JsValue> {
    Reflect::get(transformers, &JsValue::from_str("TextStreamer"))?.into_function("TextStreamer")
}

/// Load the transformers.js ES module through the loader the page installs on `window`.
///
/// The page owns the runtime's URL (the same way it owns onnxruntime-web's), and loads it lazily so the
/// bundle and its wasm only download for a session that actually opens the chat.
async fn load_runtime() -> Result<JsValue, JsValue> {
    set_module_status("llm1: loading transformers.js runtime")?;
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
    let loader = Reflect::get(window.as_ref(), &JsValue::from_str("loadTransformers"))?;
    if loader.is_undefined() || loader.is_null() {
        return Err(JsValue::from_str(
            "window.loadTransformers is unavailable -- the page did not install the transformers.js loader",
        ));
    }
    let promise = loader
        .into_function("loadTransformers")?
        .call0(window.as_ref())?
        .into_promise("loadTransformers")?;
    JsFuture::from(promise).await
}

/// Point the runtime at this origin: local weights only, and the ORT wasm pair this module vendors.
///
/// transformers.js picks its ORT wasm filenames itself at import time (a Safari build and an asyncify build
/// for everyone else) and defaults them to a CDN. Rewriting only the directory keeps upstream's choice while
/// making sure the bytes come from the ws-server, so a chat session works with no internet access at all.
fn configure_runtime(transformers: &JsValue) -> Result<(), JsValue> {
    let env = Reflect::get(transformers, &JsValue::from_str("env"))?;
    Reflect::set(&env, &JsValue::from_str("allowRemoteModels"), &JsValue::FALSE)?;
    Reflect::set(&env, &JsValue::from_str("allowLocalModels"), &JsValue::TRUE)?;
    Reflect::set(
        &env,
        &JsValue::from_str("localModelPath"),
        &JsValue::from_str(LOCAL_MODEL_PATH),
    )?;

    let backends = Reflect::get(&env, &JsValue::from_str("backends"))?;
    let onnx = Reflect::get(&backends, &JsValue::from_str("onnx"))?;
    let wasm = Reflect::get(&onnx, &JsValue::from_str("wasm"))?;
    if wasm.is_undefined() || wasm.is_null() {
        return Err(JsValue::from_str("transformers.js onnx wasm backend is unavailable"));
    }
    let defaults = Reflect::get(&wasm, &JsValue::from_str("wasmPaths"))?;
    let paths = Object::new();
    for key in ["mjs", "wasm"] {
        let default = Reflect::get(&defaults, &JsValue::from_str(key))?
            .as_string()
            .ok_or_else(|| JsValue::from_str(&format!("transformers.js set no default wasmPaths.{key}")))?;
        let file = default.rsplit('/').next().unwrap_or(&default);
        Reflect::set(
            &paths,
            &JsValue::from_str(key),
            &JsValue::from_str(&format!("{ORT_WASM_DIR}/{file}")),
        )?;
    }
    Reflect::set(&wasm, &JsValue::from_str("wasmPaths"), &paths)?;
    Ok(())
}

/// Build the text-generation pipeline, reporting weight-download progress to the page while it loads.
///
/// WebGPU is required rather than optional: the q4f16 weights this module ships are the WebGPU-friendly
/// export, and ORT's CPU backend has no usable float16 path for them. Checking `navigator.gpu` up front turns
/// "no WebGPU in this browser" into that sentence instead of an ORT session-creation error.
async fn create_generator(transformers: &JsValue) -> Result<JsValue, JsValue> {
    let navigator = web_sys::window()
        .ok_or_else(|| JsValue::from_str("No window available"))?
        .navigator();
    let gpu = Reflect::get(navigator.as_ref(), &JsValue::from_str("gpu"))?;
    if gpu.is_undefined() || gpu.is_null() {
        return Err(JsValue::from_str(
            "navigator.gpu is unavailable -- llm1 needs WebGPU to run its q4f16 weights",
        ));
    }
    set_module_status("llm1: loading model weights (first run downloads ~118 MB)")?;

    let on_progress: Box<dyn FnMut(JsValue)> = Box::new(|report: JsValue| report_load_progress(&report));
    let on_progress = Closure::wrap(on_progress);

    let options = Object::new();
    Reflect::set(&options, &JsValue::from_str("dtype"), &JsValue::from_str(MODEL_DTYPE))?;
    Reflect::set(&options, &JsValue::from_str("device"), &JsValue::from_str("webgpu"))?;
    Reflect::set(&options, &JsValue::from_str("progress_callback"), on_progress.as_ref())?;

    let pipeline = Reflect::get(transformers, &JsValue::from_str("pipeline"))?.into_function("pipeline")?;
    let created = pipeline.call3(
        &JsValue::NULL,
        &JsValue::from_str("text-generation"),
        &JsValue::from_str(MODEL_ID),
        &options,
    )?;
    JsFuture::from(created.into_promise("pipeline")?).await
}

/// Show one transformers.js load-progress report in the status textarea.
fn report_load_progress(report: &JsValue) {
    let status = Reflect::get(report, &JsValue::from_str("status"))
        .ok()
        .and_then(|value| value.as_string())
        .unwrap_or_else(|| "loading".to_owned());
    let file = Reflect::get(report, &JsValue::from_str("file"))
        .ok()
        .and_then(|value| value.as_string())
        .unwrap_or_default();
    let progress = Reflect::get(report, &JsValue::from_str("progress"))
        .ok()
        .and_then(|value| value.as_f64())
        .unwrap_or_default();
    let message = format!("llm1: {status} {file} {progress:.0}%");
    let _shown = set_textarea_value("module-output", &message);
}

/// Turn the transcript into the message array transformers.js applies the model's chat template to.
fn chat_messages(history: &[Turn]) -> Result<Array, JsValue> {
    let messages = Array::new();
    messages.push(&message_object("system", SYSTEM_PROMPT)?);
    for turn in history {
        messages.push(&message_object(&turn.role, &turn.content)?);
    }
    Ok(messages)
}

/// Build one `{ role, content }` message object.
fn message_object(role: &str, content: &str) -> Result<JsValue, JsValue> {
    let message = Object::new();
    Reflect::set(&message, &JsValue::from_str("role"), &JsValue::from_str(role))?;
    Reflect::set(&message, &JsValue::from_str("content"), &JsValue::from_str(content))?;
    Ok(message.into())
}

/// Drop the oldest turns once the transcript outgrows the context window we resend.
fn truncate_history(history: &mut Vec<Turn>) {
    let excess = history.len().saturating_sub(MAX_HISTORY_TURNS);
    if excess > 0 {
        history.drain(0..excess);
    }
}

/// Read the assistant's reply out of a text-generation pipeline result.
///
/// A chat-array input makes the pipeline echo the whole conversation back as `generated_text`, with its own
/// turn appended last -- that final `content` is the reply.
fn extract_reply(output: &JsValue) -> Result<String, JsValue> {
    let first = Reflect::get(output, &JsValue::from_f64(0.0))?;
    let generated = Reflect::get(&first, &JsValue::from_str("generated_text"))?;
    if let Some(text) = generated.as_string() {
        return Ok(text);
    }
    let turns = generated.dyn_into::<Array>().js_context("generated_text")?;
    let last = turns.get(turns.length().saturating_sub(1));
    Reflect::get(&last, &JsValue::from_str("content"))?
        .as_string()
        .ok_or_else(|| JsValue::from_str("Generated turn carried no content"))
}

/// The panel's event listeners, kept alive for the session and removed by [`Listeners::detach`].
struct Listeners {
    close: Closure<dyn FnMut()>,
    keydown: Closure<dyn FnMut(KeyboardEvent)>,
    send: Closure<dyn FnMut()>,
}

impl Listeners {
    /// Remove every listener from the panel so a second run starts from a clean page.
    fn detach(&self) {
        if let Ok(button) = element("chat-send") {
            let _removed = button
                .remove_event_listener_with_callback("click", self.send.as_ref().unchecked_ref())
                .js_context("chat-send");
        }
        if let Ok(button) = element("chat-close") {
            let _removed = button
                .remove_event_listener_with_callback("click", self.close.as_ref().unchecked_ref())
                .js_context("chat-close");
        }
        if let Ok(input) = element("chat-input") {
            let _removed = input
                .remove_event_listener_with_callback("keydown", self.keydown.as_ref().unchecked_ref())
                .js_context("chat-input");
        }
    }
}

/// Wire the panel's send button, close button and Enter key to the prompt queue.
fn attach_panel_listeners(prompts: &Rc<RefCell<VecDeque<String>>>) -> Result<Listeners, JsValue> {
    let send: Box<dyn FnMut()> = Box::new({
        let prompts = Rc::clone(prompts);
        move || queue_prompt(&prompts)
    });
    let send = Closure::wrap(send);
    element("chat-send")?.add_event_listener_with_callback("click", send.as_ref().unchecked_ref())?;

    let close: Box<dyn FnMut()> = Box::new(stop);
    let close = Closure::wrap(close);
    element("chat-close")?.add_event_listener_with_callback("click", close.as_ref().unchecked_ref())?;

    // Enter sends, Shift+Enter keeps the newline -- the convention every chat box on the web uses.
    let keydown: Box<dyn FnMut(KeyboardEvent)> = Box::new({
        let prompts = Rc::clone(prompts);
        move |event: KeyboardEvent| {
            if event.key() == "Enter" && !event.shift_key() {
                event.prevent_default();
                queue_prompt(&prompts);
            }
        }
    });
    let keydown = Closure::wrap(keydown);
    element("chat-input")?.add_event_listener_with_callback("keydown", keydown.as_ref().unchecked_ref())?;

    Ok(Listeners { close, keydown, send })
}

/// Move whatever the input holds onto the prompt queue, clearing the box; blank input is ignored.
fn queue_prompt(prompts: &Rc<RefCell<VecDeque<String>>>) {
    let Ok(input) = chat_input() else {
        return;
    };
    let prompt = input.value().trim().to_owned();
    if prompt.is_empty() {
        return;
    }
    input.set_value("");
    prompts.borrow_mut().push_back(prompt);
}

/// Append a turn to the transcript, returning the element its text lives in so it can stream in.
fn append_turn(role: &str, text: &str) -> Result<HtmlElement, JsValue> {
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| JsValue::from_str("No document available"))?;

    let turn = document.create_element("div")?;
    turn.set_class_name(&format!("chat-turn chat-turn-{role}"));
    let label = document.create_element("strong")?;
    label.set_text_content(Some(if role == "user" { "you" } else { "llm1" }));
    turn.append_child(&label)?;
    let body = document.create_element("div")?;
    body.set_class_name("chat-text");
    body.set_text_content(Some(text));
    turn.append_child(&body)?;
    chat_log()?.append_child(&turn)?;
    scroll_log_to_end();

    body.dyn_into::<HtmlElement>().js_context("chat turn body")
}

/// Keep the newest turn in view as the transcript grows.
fn scroll_log_to_end() {
    if let Ok(log_element) = chat_log() {
        log_element.set_scroll_top(log_element.scroll_height());
    }
}

/// Reveal the page's chat panel and focus the input, starting from an empty transcript.
///
/// The transcript is cleared because each run starts with an empty history: leaving the previous session's
/// turns on screen would imply the model still has them in context when it does not.
fn show_panel() -> Result<(), JsValue> {
    chat_log()?.set_text_content(None);
    chat_panel()?.set_hidden(false);
    chat_input()?.focus()
}

/// Hide the chat panel again once the session ends (best-effort; a missing element is fine).
fn hide_panel() {
    if let Ok(panel) = chat_panel() {
        panel.set_hidden(true);
    }
}

/// Look up the page's chat panel container.
fn chat_panel() -> Result<HtmlElement, JsValue> {
    element("chat-panel")?
        .dyn_into::<HtmlElement>()
        .js_context("chat-panel")
}

/// Look up the transcript container the turns are appended to.
fn chat_log() -> Result<HtmlElement, JsValue> {
    element("chat-log")?.dyn_into::<HtmlElement>().js_context("chat-log")
}

/// Look up the prompt input box.
fn chat_input() -> Result<HtmlTextAreaElement, JsValue> {
    element("chat-input")?
        .dyn_into::<HtmlTextAreaElement>()
        .js_context("chat-input")
}

/// Look up one of the panel's elements by id.
fn element(element_id: &str) -> Result<web_sys::Element, JsValue> {
    web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id(element_id))
        .ok_or_else(|| JsValue::from_str(&format!("Missing #{element_id} element")))
}

/// Log one line to the browser console with the module prefix.
fn log(message: &str) {
    let line = format!("[llm1] {message}");
    web_sys::console::log_1(&JsValue::from_str(&line));
}

/// Replace the page's module-output textarea with the chat's current status.
fn set_module_status(message: &str) -> Result<(), JsValue> {
    set_textarea_value("module-output", message)
}

/// Wait until the WebSocket client reports the connected state, or time out after ~10 seconds.
async fn wait_for_connected(client: &WsClient) -> Result<(), JsValue> {
    for _attempt in 0_u32..100 {
        if client.get_state() == "connected" {
            return Ok(());
        }
        sleep_ms(100).await?;
    }

    Err(JsValue::from_str("Timed out waiting for websocket connection"))
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

/// Sleep for `duration_ms` via the window's timer, yielding to the browser event loop.
async fn sleep_ms(duration_ms: i32) -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
    let promise = Promise::new(&mut |resolve, reject| {
        let callback = Closure::once_into_js(move || {
            let _resolved = resolve.call0(&JsValue::NULL);
        });

        if let Err(error) =
            window.set_timeout_with_callback_and_timeout_and_arguments_0(callback.unchecked_ref(), duration_ms)
        {
            let _rejected = reject.call1(&JsValue::NULL, &error);
        }
    });
    JsFuture::from(promise).await.map(et_web::ignore)
}

/// Derive the WebSocket endpoint from the page's own origin (ws:// for http, wss:// for https).
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
