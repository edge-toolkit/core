use et_web::JsCastExt;
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
        let canvas: HtmlCanvasElement = document
            .create_element("canvas")?
            .dyn_into_msg("Failed to create canvas element")?;

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

        let request_adapter: js_sys::Function = js_sys::Reflect::get(&gpu, &JsValue::from_str("requestAdapter"))?
            .dyn_into_msg("navigator.gpu.requestAdapter is not callable")?;

        let adapter_promise: js_sys::Promise = request_adapter
            .call0(&gpu)?
            .dyn_into_msg("requestAdapter did not return a Promise")?;
        let adapter = JsFuture::from(adapter_promise).await?;

        if adapter.is_null() || adapter.is_undefined() {
            info!("WebGPU probe: no adapter available");
            return Ok(WebGpuProbeResult {
                adapter_found: false,
                device_created: false,
            });
        }

        let request_device: js_sys::Function = js_sys::Reflect::get(&adapter, &JsValue::from_str("requestDevice"))?
            .dyn_into_msg("adapter.requestDevice is not callable")?;

        let device_promise: js_sys::Promise = request_device
            .call0(&adapter)?
            .dyn_into_msg("requestDevice did not return a Promise")?;
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

/// Result of a GPU matrix-multiply computation.
#[wasm_bindgen]
pub struct GpuComputeResult {
    success: bool,
    /// Time taken in milliseconds (JS `performance.now()` delta).
    elapsed_ms: f64,
    /// First element of the output matrix (C[0][0]) for spot-check.
    result_c00: f32,
}

#[wasm_bindgen]
impl GpuComputeResult {
    /// Run a 4×4 matrix multiply A×B=C on the GPU using a WebGPU compute shader.
    ///
    /// A and B are hard-coded identity-like matrices so the expected C[0][0] = 1.0.
    #[wasm_bindgen(js_name = run)]
    pub async fn run() -> Result<GpuComputeResult, JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window"))?;
        let navigator = window.navigator();
        let gpu = js_sys::Reflect::get(&navigator, &JsValue::from_str("gpu"))?;
        if gpu.is_null() || gpu.is_undefined() {
            return Ok(GpuComputeResult {
                success: false,
                elapsed_ms: 0.0,
                result_c00: 0.0,
            });
        }

        // requestAdapter
        let request_adapter: js_sys::Function = js_sys::Reflect::get(&gpu, &JsValue::from_str("requestAdapter"))?
            .dyn_into_msg("gpu.requestAdapter not callable")?;
        let adapter = JsFuture::from(request_adapter.call0(&gpu)?.dyn_into::<Promise>()?).await?;
        if adapter.is_null() || adapter.is_undefined() {
            return Ok(GpuComputeResult {
                success: false,
                elapsed_ms: 0.0,
                result_c00: 0.0,
            });
        }

        // requestDevice
        let request_device: js_sys::Function = js_sys::Reflect::get(&adapter, &JsValue::from_str("requestDevice"))?
            .dyn_into_msg("adapter.requestDevice not callable")?;
        let device = JsFuture::from(request_device.call0(&adapter)?.dyn_into::<Promise>()?).await?;
        if device.is_null() || device.is_undefined() {
            return Ok(GpuComputeResult {
                success: false,
                elapsed_ms: 0.0,
                result_c00: 0.0,
            });
        }

        // Catch any silent WebGPU validation errors.
        let push_error_scope =
            js_sys::Reflect::get(&device, &JsValue::from_str("pushErrorScope"))?.dyn_into::<js_sys::Function>()?;
        push_error_scope.call1(&device, &JsValue::from_str("validation"))?;

        // 4×4 matrices stored as f32 arrays (row-major).
        // A = identity, B = identity → C = identity, so C[0][0] = 1.0.
        #[rustfmt::skip]
        let a: [f32; 16] = [
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0,
        ];
        #[rustfmt::skip]
        let b: [f32; 16] = [
            2.0, 0.0, 0.0, 0.0,
            0.0, 2.0, 0.0, 0.0,
            0.0, 0.0, 2.0, 0.0,
            0.0, 0.0, 0.0, 2.0,
        ];

        let matrix_bytes = (16 * 4) as f64; // 16 f32 = 64 bytes

        // Helper: create a GPUBuffer from a &[f32].
        let create_buffer_with_data = |data: &[f32], usage: u32| -> Result<JsValue, JsValue> {
            let buf_desc = js_sys::Object::new();
            js_sys::Reflect::set(&buf_desc, &JsValue::from_str("size"), &JsValue::from_f64(matrix_bytes))?;
            js_sys::Reflect::set(&buf_desc, &JsValue::from_str("usage"), &JsValue::from_f64(usage as f64))?;
            js_sys::Reflect::set(
                &buf_desc,
                &JsValue::from_str("mappedAtCreation"),
                &JsValue::from_bool(true),
            )?;
            let create_buffer =
                js_sys::Reflect::get(&device, &JsValue::from_str("createBuffer"))?.dyn_into::<js_sys::Function>()?;
            let buf = create_buffer.call1(&device, &buf_desc)?;
            // getMappedRange → write data → unmap
            let get_mapped =
                js_sys::Reflect::get(&buf, &JsValue::from_str("getMappedRange"))?.dyn_into::<js_sys::Function>()?;
            let mapped = get_mapped.call0(&buf)?;
            let mapped_array = js_sys::Float32Array::new(&mapped);
            mapped_array.copy_from(data);
            let unmap = js_sys::Reflect::get(&buf, &JsValue::from_str("unmap"))?.dyn_into::<js_sys::Function>()?;
            unmap.call0(&buf)?;
            Ok(buf)
        };

        // GPUBuffer usage flags (from the WebGPU spec).
        const MAP_READ: u32 = 0x0001;
        const COPY_SRC: u32 = 0x0004;
        const COPY_DST: u32 = 0x0008;
        const STORAGE: u32 = 0x0080;

        let buf_a = create_buffer_with_data(&a, STORAGE)?;
        let buf_b = create_buffer_with_data(&b, STORAGE)?;

        // Output buffer (STORAGE | COPY_SRC so we can copy to a readback buffer).
        let out_desc = js_sys::Object::new();
        js_sys::Reflect::set(&out_desc, &JsValue::from_str("size"), &JsValue::from_f64(matrix_bytes))?;
        js_sys::Reflect::set(
            &out_desc,
            &JsValue::from_str("usage"),
            &JsValue::from_f64((STORAGE | COPY_SRC) as f64),
        )?;
        let create_buffer_fn =
            js_sys::Reflect::get(&device, &JsValue::from_str("createBuffer"))?.dyn_into::<js_sys::Function>()?;
        let buf_out = create_buffer_fn.call1(&device, &out_desc)?;

        // Readback buffer (COPY_DST | MAP_READ).
        let rb_desc = js_sys::Object::new();
        js_sys::Reflect::set(&rb_desc, &JsValue::from_str("size"), &JsValue::from_f64(matrix_bytes))?;
        js_sys::Reflect::set(
            &rb_desc,
            &JsValue::from_str("usage"),
            &JsValue::from_f64((COPY_DST | MAP_READ) as f64),
        )?;
        let buf_readback = create_buffer_fn.call1(&device, &rb_desc)?;

        // WGSL compute shader: 4×4 matrix multiply.
        let wgsl = r#"
@group(0) @binding(0) var<storage, read>       matA : array<f32, 16>;
@group(0) @binding(1) var<storage, read>       matB : array<f32, 16>;
@group(0) @binding(2) var<storage, read_write> matC : array<f32, 16>;

@compute @workgroup_size(4, 4)
fn main(@builtin(global_invocation_id) gid : vec3<u32>) {
    let row = gid.y;
    let col = gid.x;
    var sum : f32 = 0.0;
    for (var k : u32 = 0u; k < 4u; k = k + 1u) {
        sum = sum + matA[row * 4u + k] * matB[k * 4u + col];
    }
    matC[row * 4u + col] = sum;
}
"#;

        // createShaderModule
        let shader_desc = js_sys::Object::new();
        js_sys::Reflect::set(&shader_desc, &JsValue::from_str("code"), &JsValue::from_str(wgsl))?;
        let create_shader =
            js_sys::Reflect::get(&device, &JsValue::from_str("createShaderModule"))?.dyn_into::<js_sys::Function>()?;
        let shader = create_shader.call1(&device, &shader_desc)?;

        // createComputePipelineAsync with layout:"auto" — browser derives BGL from shader
        let compute_stage = js_sys::Object::new();
        js_sys::Reflect::set(&compute_stage, &JsValue::from_str("module"), &shader)?;
        js_sys::Reflect::set(
            &compute_stage,
            &JsValue::from_str("entryPoint"),
            &JsValue::from_str("main"),
        )?;
        let cp_desc = js_sys::Object::new();
        js_sys::Reflect::set(&cp_desc, &JsValue::from_str("layout"), &JsValue::from_str("auto"))?;
        js_sys::Reflect::set(&cp_desc, &JsValue::from_str("compute"), &compute_stage)?;
        let create_cp = js_sys::Reflect::get(&device, &JsValue::from_str("createComputePipelineAsync"))?
            .dyn_into::<js_sys::Function>()?;
        let pipeline = JsFuture::from(create_cp.call1(&device, &cp_desc)?.dyn_into::<Promise>()?).await?;

        // getBindGroupLayout(0) from the pipeline
        let get_bgl = js_sys::Reflect::get(&pipeline, &JsValue::from_str("getBindGroupLayout"))?
            .dyn_into::<js_sys::Function>()?;
        let bgl = get_bgl.call1(&pipeline, &JsValue::from_f64(0.0))?;

        // createBindGroup
        let make_bg_entry = |binding: u32, buf: &JsValue| -> Result<js_sys::Object, JsValue> {
            let entry = js_sys::Object::new();
            js_sys::Reflect::set(
                &entry,
                &JsValue::from_str("binding"),
                &JsValue::from_f64(binding as f64),
            )?;
            let resource = js_sys::Object::new();
            js_sys::Reflect::set(&resource, &JsValue::from_str("buffer"), buf)?;
            js_sys::Reflect::set(&entry, &JsValue::from_str("resource"), &resource)?;
            Ok(entry)
        };
        let bg_entries = js_sys::Array::new();
        bg_entries.push(&make_bg_entry(0, &buf_a)?.into());
        bg_entries.push(&make_bg_entry(1, &buf_b)?.into());
        bg_entries.push(&make_bg_entry(2, &buf_out)?.into());
        let bg_desc = js_sys::Object::new();
        js_sys::Reflect::set(&bg_desc, &JsValue::from_str("layout"), &bgl)?;
        js_sys::Reflect::set(&bg_desc, &JsValue::from_str("entries"), &bg_entries)?;
        let create_bg =
            js_sys::Reflect::get(&device, &JsValue::from_str("createBindGroup"))?.dyn_into::<js_sys::Function>()?;
        let bind_group = create_bg.call1(&device, &bg_desc)?;

        // Record and submit commands.
        let perf = js_sys::Reflect::get(&window, &JsValue::from_str("performance"))?;
        let now_fn = js_sys::Reflect::get(&perf, &JsValue::from_str("now"))?.dyn_into::<js_sys::Function>()?;
        let t0 = now_fn.call0(&perf)?.as_f64().unwrap_or(0.0);

        let create_encoder = js_sys::Reflect::get(&device, &JsValue::from_str("createCommandEncoder"))?
            .dyn_into::<js_sys::Function>()?;
        let encoder = create_encoder.call0(&device)?;

        let begin_compute =
            js_sys::Reflect::get(&encoder, &JsValue::from_str("beginComputePass"))?.dyn_into::<js_sys::Function>()?;
        let pass = begin_compute.call0(&encoder)?;

        let set_pipeline =
            js_sys::Reflect::get(&pass, &JsValue::from_str("setPipeline"))?.dyn_into::<js_sys::Function>()?;
        set_pipeline.call1(&pass, &pipeline)?;

        let set_bg = js_sys::Reflect::get(&pass, &JsValue::from_str("setBindGroup"))?.dyn_into::<js_sys::Function>()?;
        set_bg.call2(&pass, &JsValue::from_f64(0.0), &bind_group)?;

        let dispatch =
            js_sys::Reflect::get(&pass, &JsValue::from_str("dispatchWorkgroups"))?.dyn_into::<js_sys::Function>()?;
        dispatch.call2(&pass, &JsValue::from_f64(1.0), &JsValue::from_f64(1.0))?;

        let end_pass = js_sys::Reflect::get(&pass, &JsValue::from_str("end"))?.dyn_into::<js_sys::Function>()?;
        end_pass.call0(&pass)?;

        // Copy output → readback buffer.
        let copy_buf =
            js_sys::Reflect::get(&encoder, &JsValue::from_str("copyBufferToBuffer"))?.dyn_into::<js_sys::Function>()?;
        copy_buf.call5(
            &encoder,
            &buf_out,
            &JsValue::from_f64(0.0),
            &buf_readback,
            &JsValue::from_f64(0.0),
            &JsValue::from_f64(matrix_bytes),
        )?;

        let finish = js_sys::Reflect::get(&encoder, &JsValue::from_str("finish"))?.dyn_into::<js_sys::Function>()?;
        let cmd_buf = finish.call0(&encoder)?;

        let queue = js_sys::Reflect::get(&device, &JsValue::from_str("queue"))?;
        let submit = js_sys::Reflect::get(&queue, &JsValue::from_str("submit"))?.dyn_into::<js_sys::Function>()?;
        let cmds = js_sys::Array::new();
        cmds.push(&cmd_buf);
        submit.call1(&queue, &cmds)?;

        // Pop error scope — surface any validation error before attempting mapAsync.
        let pop_error_scope =
            js_sys::Reflect::get(&device, &JsValue::from_str("popErrorScope"))?.dyn_into::<js_sys::Function>()?;
        let gpu_error = JsFuture::from(pop_error_scope.call0(&device)?.dyn_into::<Promise>()?).await?;
        if !gpu_error.is_null() && !gpu_error.is_undefined() {
            let msg = js_sys::Reflect::get(&gpu_error, &JsValue::from_str("message"))
                .ok()
                .and_then(|v| v.as_string())
                .unwrap_or_else(|| "unknown GPU validation error".to_string());
            return Err(JsValue::from_str(&format!("WebGPU validation error: {}", msg)));
        }

        // Map readback buffer and read C[0][0].
        let map_async =
            js_sys::Reflect::get(&buf_readback, &JsValue::from_str("mapAsync"))?.dyn_into::<js_sys::Function>()?;
        JsFuture::from(
            map_async
                .call1(&buf_readback, &JsValue::from_f64(1.0))?
                .dyn_into::<Promise>()?,
        )
        .await?;

        let t1 = now_fn.call0(&perf)?.as_f64().unwrap_or(0.0);

        let get_mapped = js_sys::Reflect::get(&buf_readback, &JsValue::from_str("getMappedRange"))?
            .dyn_into::<js_sys::Function>()?;
        let mapped = get_mapped.call0(&buf_readback)?;
        let result_array = js_sys::Float32Array::new(&mapped);
        let result_c00 = result_array.get_index(0);

        let unmap = js_sys::Reflect::get(&buf_readback, &JsValue::from_str("unmap"))?.dyn_into::<js_sys::Function>()?;
        unmap.call0(&buf_readback)?;

        Ok(GpuComputeResult {
            success: true,
            elapsed_ms: t1 - t0,
            result_c00,
        })
    }

    #[wasm_bindgen(js_name = success)]
    pub fn success(&self) -> bool {
        self.success
    }

    #[wasm_bindgen(js_name = elapsedMs)]
    pub fn elapsed_ms(&self) -> f64 {
        self.elapsed_ms
    }

    /// C[0][0] of the output matrix. For identity × 2×identity the expected value is 2.0.
    #[wasm_bindgen(js_name = resultC00)]
    pub fn result_c00(&self) -> f32 {
        self.result_c00
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

    let adapter_promise: js_sys::Promise = request_adapter
        .call0(&gpu)?
        .dyn_into_msg("requestAdapter did not return a Promise")?;
    let adapter = JsFuture::from(adapter_promise).await?;

    if adapter.is_null() || adapter.is_undefined() {
        return Ok(None);
    }

    let info_object = if let Some(request_adapter_info) =
        js_sys::Reflect::get(&adapter, &JsValue::from_str("requestAdapterInfo"))
            .ok()
            .and_then(|value| value.dyn_into::<js_sys::Function>().ok())
    {
        let info_promise: js_sys::Promise = request_adapter_info
            .call0(&adapter)?
            .dyn_into_msg("requestAdapterInfo did not return a Promise")?;
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
    let canvas: HtmlCanvasElement = document
        .create_element("canvas")?
        .dyn_into_msg("Failed to create canvas element")?;

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
        log(&format!("websocket connected with agent_id={}", client.get_agent_id()))?;

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

        log("running GPU matrix multiply (4×4)")?;
        let compute = GpuComputeResult::run().await?;
        if compute.success() {
            let expected = 2.0_f32;
            if (compute.result_c00() - expected).abs() < 1e-4 {
                log("GPU compute: ok")?;
            } else {
                log(&format!(
                    "GPU compute: WRONG result C[0][0]={} (expected {})",
                    compute.result_c00(),
                    expected
                ))?;
            }
        } else {
            log("GPU compute: skipped (WebGPU unavailable)")?;
        }

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
                },
                "gpu_compute": {
                    "success": compute.success(),
                    "elapsed_ms": compute.elapsed_ms(),
                    "result_c00": compute.result_c00(),
                }
            }),
        )?;

        set_module_status(&format!(
            "graphics-info: detected\nGPU: {}\nRenderer: {}\nWebGPU: {}\nCompute: {}",
            gpu.vendor(),
            gpu.renderer(),
            if probe.device_created() {
                "Available"
            } else {
                "Unavailable"
            },
            if compute.success() {
                format!("C[0][0]={:.1} in {:.2}ms", compute.result_c00(), compute.elapsed_ms())
            } else {
                "skipped".to_string()
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
