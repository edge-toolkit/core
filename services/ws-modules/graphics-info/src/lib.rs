use et_ws_wasm_agent::{WsClient, WsClientConfig, set_textarea_value};
use js_sys::{Promise, Reflect};
use serde_json::json;
use tracing::info;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::HtmlCanvasElement;

#[wasm_bindgen]
pub struct GraphicsSupport {
    webgl_supported: bool,
    webgl2_supported: bool,
    webgpu_supported: bool,
    webnn_supported: bool,
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
            .dyn_into::<HtmlCanvasElement>()
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
pub struct WebGpuProbeResult {
    adapter_found: bool,
    device_created: bool,
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
pub struct GpuInfo {
    vendor: String,
    renderer: String,
    architecture: String,
    description: String,
    source: String,
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
        .dyn_into::<HtmlCanvasElement>()
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

#[wasm_bindgen(start)]
pub fn init() {
    let _ = tracing_wasm::try_set_as_global_default();
    info!("graphics-info module initialized");
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
    false
}

#[wasm_bindgen]
pub async fn run() -> Result<(), JsValue> {
    set_module_status("graphics-info: entered run()")?;
    log("entered run()")?;

    let outcome = async {
        let ws_url = websocket_url()?;
        let mut client = WsClient::new(WsClientConfig::new(ws_url));
        client.connect()?;
        wait_for_connected(&client).await?;
        log(&format!("websocket connected with agent_id={}", client.get_client_id()))?;

        log("detecting graphics support")?;
        let support = GraphicsSupport::detect()?;
        log(&format!(
            "graphics support: webgl={} webgl2={} webgpu={} webnn={}",
            support.webgl_supported(),
            support.webgl2_supported(),
            support.webgpu_supported(),
            support.webnn_supported()
        ))?;

        log("probing WebGPU")?;
        let probe = WebGpuProbeResult::test().await?;
        log(&format!(
            "WebGPU probe: adapter={} device={}",
            probe.adapter_found(),
            probe.device_created()
        ))?;

        log("detecting GPU info")?;
        let gpu = GpuInfo::detect().await?;
        log(&format!(
            "GPU info: vendor={} renderer={} architecture={} source={}",
            gpu.vendor(),
            gpu.renderer(),
            gpu.architecture(),
            gpu.source()
        ))?;

        client.send_client_event(
            "graphics",
            "info_detected",
            json!({
                "support": {
                    "webgl": support.webgl_supported(),
                    "webgl2": support.webgl2_supported(),
                    "webgpu": support.webgpu_supported(),
                    "webnn": support.webnn_supported(),
                },
                "webgpu_probe": {
                    "adapter_found": probe.adapter_found(),
                    "device_created": probe.device_created(),
                },
                "gpu": {
                    "vendor": gpu.vendor(),
                    "renderer": gpu.renderer(),
                    "architecture": gpu.architecture(),
                    "description": gpu.description(),
                    "source": gpu.source(),
                }
            }),
        )?;

        set_module_status(&format!(
            "graphics-info: detected\nGPU: {}\nRenderer: {}\nWebGPU: {}",
            gpu.vendor(),
            gpu.renderer(),
            if probe.device_created() {
                "Available"
            } else {
                "Unavailable"
            }
        ))?;

        client.disconnect();
        Ok(())
    }
    .await;

    if let Err(error) = &outcome {
        let message = describe_js_error(error);
        let _ = set_module_status(&format!("graphics-info: error\n{}", message));
        let _ = log(&format!("error: {}", message));
    }

    outcome
}

fn log(message: &str) -> Result<(), JsValue> {
    let line = format!("[graphics-info] {}", message);
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
