#![expect(
    clippy::float_arithmetic,
    clippy::future_not_send,
    clippy::single_call_fn,
    reason = "browser WASM module: JsFuture is !Send; GPU benchmark uses float math; pipeline helpers are single-use"
)]

use et_web::{JsCastExt as _, JsFunctionExt as _, JsPromiseExt as _, describe_js_error, websocket_url};
use et_ws_wasm_agent::{WsClient, WsClientConfig, set_textarea_value, wait_for_connected};
use serde_json::json;
use tracing::info;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::HtmlCanvasElement;

// GPUBuffer usage flags (from the WebGPU spec).
const MAP_READ: u32 = 0x0001;
const COPY_SRC: u32 = 0x0004;
const COPY_DST: u32 = 0x0008;
const STORAGE: u32 = 0x0080;

/// Bytes for a 4x4 f32 matrix.
const MATRIX_BYTES: f64 = 64.0;

/// WGSL compute shader: 4x4 matrix multiply.
const MATMUL_WGSL: &str = "
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
";

/// Bytes for the numeric probes' eight f32 inputs.
const PROBE_INPUT_BYTES: f64 = 32.0;

/// Bytes for one numeric probe's u32 result.
const PROBE_OUTPUT_BYTES: f64 = 4.0;

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
    #[expect(
        clippy::similar_names,
        reason = "webgl and webgl2 are WebGL spec spellings; they must match the struct fields and JS names"
    )]
    pub fn detect() -> Result<Self, JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
        let canvas = create_canvas(&window)?;

        let webgl_supported = canvas.get_context("webgl")?.is_some();
        let webgl2_supported = canvas.get_context("webgl2")?.is_some();
        let webgpu_supported = js_sys::Reflect::get(&window.navigator(), &JsValue::from_str("gpu"))?.is_object();
        let webnn_supported = js_sys::Reflect::get(&window.navigator(), &JsValue::from_str("ml"))?.is_object();

        info!(
            "Graphics support detected: webgl={} webgl2={} webgpu={} webnn={}",
            webgl_supported, webgl2_supported, webgpu_supported, webnn_supported
        );

        Ok(Self {
            webgl_supported,
            webgl2_supported,
            webgpu_supported,
            webnn_supported,
        })
    }

    #[must_use]
    #[wasm_bindgen(js_name = webglSupported)]
    #[expect(
        clippy::missing_const_for_fn,
        reason = "wasm_bindgen rejects const fns; methods cannot be marked const"
    )]
    pub fn webgl_supported(&self) -> bool {
        self.webgl_supported
    }

    #[must_use]
    #[wasm_bindgen(js_name = webgl2Supported)]
    #[expect(
        clippy::missing_const_for_fn,
        reason = "wasm_bindgen rejects const fns; methods cannot be marked const"
    )]
    pub fn webgl2_supported(&self) -> bool {
        self.webgl2_supported
    }

    #[must_use]
    #[wasm_bindgen(js_name = webgpuSupported)]
    #[expect(
        clippy::missing_const_for_fn,
        reason = "wasm_bindgen rejects const fns; methods cannot be marked const"
    )]
    pub fn webgpu_supported(&self) -> bool {
        self.webgpu_supported
    }

    #[must_use]
    #[wasm_bindgen(js_name = webnnSupported)]
    #[expect(
        clippy::missing_const_for_fn,
        reason = "wasm_bindgen rejects const fns; methods cannot be marked const"
    )]
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
    #[expect(
        clippy::cognitive_complexity,
        reason = "one probe per graphics API, each logging what it found"
    )]
    pub async fn test() -> Result<Self, JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
        let navigator = window.navigator();
        let gpu = js_sys::Reflect::get(&navigator, &JsValue::from_str("gpu"))?;

        if gpu.is_null() || gpu.is_undefined() {
            return Ok(Self {
                adapter_found: false,
                device_created: false,
            });
        }

        let request_adapter = js_sys::Reflect::get(&gpu, &JsValue::from_str("requestAdapter"))?
            .into_function("navigator.gpu.requestAdapter")?;

        let adapter_promise = request_adapter.call0(&gpu)?.into_promise("requestAdapter")?;
        let adapter = JsFuture::from(adapter_promise).await?;

        if adapter.is_null() || adapter.is_undefined() {
            info!("WebGPU probe: no adapter available");
            return Ok(Self {
                adapter_found: false,
                device_created: false,
            });
        }

        let request_device = js_sys::Reflect::get(&adapter, &JsValue::from_str("requestDevice"))?
            .into_function("adapter.requestDevice")?;

        let device_promise = request_device.call0(&adapter)?.into_promise("requestDevice")?;
        let device = JsFuture::from(device_promise).await?;

        let device_created = !device.is_null() && !device.is_undefined();
        info!(
            "WebGPU probe completed: adapter_found=true device_created={}",
            device_created
        );

        Ok(Self {
            adapter_found: true,
            device_created,
        })
    }

    #[must_use]
    #[wasm_bindgen(js_name = adapterFound)]
    #[expect(
        clippy::missing_const_for_fn,
        reason = "wasm_bindgen rejects const fns; methods cannot be marked const"
    )]
    pub fn adapter_found(&self) -> bool {
        self.adapter_found
    }

    #[must_use]
    #[wasm_bindgen(js_name = deviceCreated)]
    #[expect(
        clippy::missing_const_for_fn,
        reason = "wasm_bindgen rejects const fns; methods cannot be marked const"
    )]
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
    /// First element of the output matrix (`C[0][0]`) for spot-check.
    result_c00: f32,
}

/// Call `target[name]` with `args`, returning what it returns.
fn call_method(target: &JsValue, name: &str, args: &[&JsValue]) -> Result<JsValue, JsValue> {
    let method = js_sys::Reflect::get(target, &JsValue::from_str(name))?.into_function(name)?;
    method.apply(target, &args.iter().copied().collect())
}

/// Call `target[name]` with `args` for its effect alone.
fn invoke(target: &JsValue, name: &str, args: &[&JsValue]) -> Result<(), JsValue> {
    let _returned: JsValue = call_method(target, name, args)?;
    Ok(())
}

/// Call `target[name]` with `args` and await the promise it returns.
async fn await_method(target: &JsValue, name: &str, args: &[&JsValue]) -> Result<JsValue, JsValue> {
    JsFuture::from(call_method(target, name, args)?.into_promise(name)?).await
}

/// A plain object holding `entries`, the shape every WebGPU descriptor takes.
fn descriptor(entries: &[(&str, &JsValue)]) -> Result<JsValue, JsValue> {
    let object = js_sys::Object::new();
    for &(key, value) in entries {
        let _: bool = js_sys::Reflect::set(&object, &JsValue::from_str(key), value)?;
    }
    Ok(object.into())
}

/// Request the default WebGPU device.
///
/// The answer is `null` or `undefined` where the browser has no WebGPU, no adapter, or no device to give.
async fn request_gpu_device(window: &web_sys::Window) -> Result<JsValue, JsValue> {
    let gpu = js_sys::Reflect::get(&window.navigator(), &JsValue::from_str("gpu"))?;
    if gpu.is_null() || gpu.is_undefined() {
        return Ok(gpu);
    }
    let adapter = await_method(&gpu, "requestAdapter", &[]).await?;
    if adapter.is_null() || adapter.is_undefined() {
        return Ok(adapter);
    }
    await_method(&adapter, "requestDevice", &[]).await
}

/// Build a `GPUBuffer` of `size` bytes with the given `usage` flags.
fn create_buffer(device: &JsValue, size: f64, usage: u32) -> Result<JsValue, JsValue> {
    let desc = descriptor(&[
        ("size", &JsValue::from_f64(size)),
        ("usage", &JsValue::from_f64(f64::from(usage))),
    ])?;
    call_method(device, "createBuffer", &[&desc])
}

/// Build a `GPUBuffer` of `size` bytes holding the supplied f32 data, with the given `usage` flags.
fn create_buffer_with_data(device: &JsValue, data: &[f32], size: f64, usage: u32) -> Result<JsValue, JsValue> {
    let desc = descriptor(&[
        ("size", &JsValue::from_f64(size)),
        ("usage", &JsValue::from_f64(f64::from(usage))),
        ("mappedAtCreation", &JsValue::from_bool(true)),
    ])?;
    let buf = call_method(device, "createBuffer", &[&desc])?;
    js_sys::Float32Array::new(&call_method(&buf, "getMappedRange", &[])?).copy_from(data);
    invoke(&buf, "unmap", &[])?;
    Ok(buf)
}

/// A `GPUBindGroupEntry` placing the whole of `buf` at `binding`.
fn make_bind_group_entry(binding: u32, buf: &JsValue) -> Result<JsValue, JsValue> {
    let resource = descriptor(&[("buffer", buf)])?;
    descriptor(&[
        ("binding", &JsValue::from_f64(f64::from(binding))),
        ("resource", &resource),
    ])
}

/// The buffers one matrix multiply runs over: its two inputs, its output, and the readback copy of the output.
struct MatmulBuffers {
    input_a: JsValue,
    input_b: JsValue,
    output: JsValue,
    readback: JsValue,
}

/// Create the multiply's buffers, with A the identity and B twice the identity, so C is 2x identity.
fn create_matmul_buffers(device: &JsValue) -> Result<MatmulBuffers, JsValue> {
    // 4x4 matrices stored as f32 arrays (row-major).
    #[rustfmt::skip]
    let mat_a: [f32; 16] = [
        1.0, 0.0, 0.0, 0.0,
        0.0, 1.0, 0.0, 0.0,
        0.0, 0.0, 1.0, 0.0,
        0.0, 0.0, 0.0, 1.0,
    ];
    #[rustfmt::skip]
    let mat_b: [f32; 16] = [
        2.0, 0.0, 0.0, 0.0,
        0.0, 2.0, 0.0, 0.0,
        0.0, 0.0, 2.0, 0.0,
        0.0, 0.0, 0.0, 2.0,
    ];
    Ok(MatmulBuffers {
        input_a: create_buffer_with_data(device, &mat_a, MATRIX_BYTES, STORAGE)?,
        input_b: create_buffer_with_data(device, &mat_b, MATRIX_BYTES, STORAGE)?,
        output: create_buffer(device, MATRIX_BYTES, STORAGE | COPY_SRC)?,
        readback: create_buffer(device, MATRIX_BYTES, COPY_DST | MAP_READ)?,
    })
}

/// Compile `wgsl` into a compute pipeline whose entry point is `main`.
async fn create_compute_pipeline(device: &JsValue, wgsl: &str) -> Result<JsValue, JsValue> {
    let shader_desc = descriptor(&[("code", &JsValue::from_str(wgsl))])?;
    let shader = call_method(device, "createShaderModule", &[&shader_desc])?;
    let compute_stage = descriptor(&[("module", &shader), ("entryPoint", &JsValue::from_str("main"))])?;
    let pipeline_desc = descriptor(&[("layout", &JsValue::from_str("auto")), ("compute", &compute_stage)])?;
    await_method(device, "createComputePipelineAsync", &[&pipeline_desc]).await
}

/// Bind `buffers`, in order, to the pipeline's storage bindings, the first at binding 0.
fn create_bind_group(device: &JsValue, pipeline: &JsValue, buffers: &[&JsValue]) -> Result<JsValue, JsValue> {
    let layout = call_method(pipeline, "getBindGroupLayout", &[&JsValue::from_f64(0.0)])?;
    let entry_list = js_sys::Array::new();
    for (binding, buffer) in (0_u32..).zip(buffers) {
        let _length: u32 = entry_list.push(&make_bind_group_entry(binding, buffer)?);
    }
    let entries: JsValue = entry_list.into();
    let bind_group_desc = descriptor(&[("layout", &layout), ("entries", &entries)])?;
    call_method(device, "createBindGroup", &[&bind_group_desc])
}

/// Encode one compute pass and the copy of `size` bytes of `output` into `readback`, and submit them.
fn submit_compute(
    device: &JsValue,
    pipeline: &JsValue,
    bind_group: &JsValue,
    output: &JsValue,
    readback: &JsValue,
    size: f64,
) -> Result<(), JsValue> {
    let zero = JsValue::from_f64(0.0);
    let one = JsValue::from_f64(1.0);
    let encoder = call_method(device, "createCommandEncoder", &[])?;
    let pass = call_method(&encoder, "beginComputePass", &[])?;
    invoke(&pass, "setPipeline", &[pipeline])?;
    invoke(&pass, "setBindGroup", &[&zero, bind_group])?;
    invoke(&pass, "dispatchWorkgroups", &[&one, &one])?;
    invoke(&pass, "end", &[])?;
    let size = JsValue::from_f64(size);
    invoke(&encoder, "copyBufferToBuffer", &[output, &zero, readback, &zero, &size])?;
    let commands: JsValue = js_sys::Array::of1(&call_method(&encoder, "finish", &[])?).into();
    let queue = js_sys::Reflect::get(device, &JsValue::from_str("queue"))?;
    invoke(&queue, "submit", &[&commands])
}

/// Fail with the validation error the device's error scope caught, when it caught one.
async fn check_validation(device: &JsValue) -> Result<(), JsValue> {
    let gpu_error = await_method(device, "popErrorScope", &[]).await?;
    if gpu_error.is_null() || gpu_error.is_undefined() {
        return Ok(());
    }
    let msg = js_sys::Reflect::get(&gpu_error, &JsValue::from_str("message"))
        .ok()
        .and_then(|value| value.as_string())
        .unwrap_or_else(|| "unknown GPU validation error".to_string());
    Err(JsValue::from_str(&format!("WebGPU validation error: {msg}")))
}

/// `performance.now()`, or 0 when it answers with something other than a number.
fn performance_now(performance: &JsValue) -> Result<f64, JsValue> {
    Ok(call_method(performance, "now", &[])?.as_f64().unwrap_or(0.0_f64))
}

/// Submit the compute pass, fail on any validation error it raised, and wait for `readback` to hold the output.
///
/// Returns the mapped range of `readback`, which the caller reads and then unmaps.
async fn compute_into_readback(
    device: &JsValue,
    pipeline: &JsValue,
    bind_group: &JsValue,
    output: &JsValue,
    readback: &JsValue,
    size: f64,
) -> Result<JsValue, JsValue> {
    submit_compute(device, pipeline, bind_group, output, readback, size)?;
    check_validation(device).await?;
    let _map_result: JsValue = await_method(readback, "mapAsync", &[&JsValue::from_f64(1.0)]).await?;
    call_method(readback, "getMappedRange", &[])
}

/// Run the multiply on `device`, returning how long it took in milliseconds and the result's top-left element.
///
/// The time covers submission through the readback being mapped, so it includes the wait for the GPU.
async fn run_gpu_compute_inner(device: &JsValue, window: &web_sys::Window) -> Result<(f64, f32), JsValue> {
    // Catch any silent WebGPU validation errors.
    invoke(device, "pushErrorScope", &[&JsValue::from_str("validation")])?;
    let buffers = create_matmul_buffers(device)?;
    let pipeline = create_compute_pipeline(device, MATMUL_WGSL).await?;
    let inputs = [&buffers.input_a, &buffers.input_b, &buffers.output];
    let bind_group = create_bind_group(device, &pipeline, &inputs)?;

    let performance = js_sys::Reflect::get(window, &JsValue::from_str("performance"))?;
    let started = performance_now(&performance)?;
    let mapped = compute_into_readback(
        device,
        &pipeline,
        &bind_group,
        &buffers.output,
        &buffers.readback,
        MATRIX_BYTES,
    )
    .await?;
    let finished = performance_now(&performance)?;

    let result_c00 = js_sys::Float32Array::new(&mapped).get_index(0);
    invoke(&buffers.readback, "unmap", &[])?;
    Ok((finished - started, result_c00))
}

/// Evaluate one numeric probe's shader on `device`, returning the bits of its result.
async fn run_probe_shader(device: &JsValue, wgsl: &str) -> Result<u32, JsValue> {
    let pipeline = create_compute_pipeline(device, wgsl).await?;
    invoke(device, "pushErrorScope", &[&JsValue::from_str("validation")])?;
    let input = create_buffer_with_data(device, &et_gpu_probe::INPUTS, PROBE_INPUT_BYTES, STORAGE)?;
    let output = create_buffer(device, PROBE_OUTPUT_BYTES, STORAGE | COPY_SRC)?;
    let readback = create_buffer(device, PROBE_OUTPUT_BYTES, COPY_DST | MAP_READ)?;
    let bind_group = create_bind_group(device, &pipeline, &[&input, &output])?;

    let mapped = compute_into_readback(device, &pipeline, &bind_group, &output, &readback, PROBE_OUTPUT_BYTES).await?;
    let bits = js_sys::Uint32Array::new(&mapped).get_index(0);
    invoke(&readback, "unmap", &[])?;
    Ok(bits)
}

#[wasm_bindgen]
impl GpuComputeResult {
    /// Run a 4x4 matrix multiply AxB=C on the GPU using a WebGPU compute shader.
    #[wasm_bindgen(js_name = run)]
    pub async fn run() -> Result<Self, JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window"))?;
        let device = request_gpu_device(&window).await?;
        if device.is_null() || device.is_undefined() {
            return Ok(Self {
                success: false,
                elapsed_ms: 0.0,
                result_c00: 0.0,
            });
        }

        let (elapsed_ms, result_c00) = run_gpu_compute_inner(&device, &window).await?;

        Ok(Self {
            success: true,
            elapsed_ms,
            result_c00,
        })
    }

    #[must_use]
    #[wasm_bindgen(js_name = success)]
    #[expect(
        clippy::missing_const_for_fn,
        reason = "wasm_bindgen rejects const fns; methods cannot be marked const"
    )]
    pub fn success(&self) -> bool {
        self.success
    }

    #[must_use]
    #[wasm_bindgen(js_name = elapsedMs)]
    #[expect(
        clippy::missing_const_for_fn,
        reason = "wasm_bindgen rejects const fns; methods cannot be marked const"
    )]
    pub fn elapsed_ms(&self) -> f64 {
        self.elapsed_ms
    }

    /// `C[0][0]` of the output matrix. For identity x 2xidentity the expected value is 2.0.
    #[must_use]
    #[wasm_bindgen(js_name = resultC00)]
    #[expect(
        clippy::missing_const_for_fn,
        reason = "wasm_bindgen rejects const fns; methods cannot be marked const"
    )]
    pub fn result_c00(&self) -> f32 {
        self.result_c00
    }
}

/// A canvas element that is not attached to the page, to ask a graphics context of.
fn create_canvas(window: &web_sys::Window) -> Result<HtmlCanvasElement, JsValue> {
    window
        .document()
        .ok_or_else(|| JsValue::from_str("No document available"))?
        .create_element("canvas")?
        .dyn_into_msg("Failed to create canvas element")
}

/// A WebGL 2 context on a one-pixel canvas, `null` where the browser has none to give.
///
/// The drawing buffer keeps its alpha unpremultiplied and is not antialiased, so the one pixel a probe draws
/// reads back as exactly the four bytes the shader wrote.
fn create_probe_context(window: &web_sys::Window) -> Result<JsValue, JsValue> {
    let canvas = create_canvas(window)?;
    canvas.set_width(1);
    canvas.set_height(1);
    let options = descriptor(&[
        ("antialias", &JsValue::FALSE),
        ("premultipliedAlpha", &JsValue::FALSE),
        ("preserveDrawingBuffer", &JsValue::TRUE),
    ])?;
    let canvas_value: &JsValue = canvas.as_ref();
    call_method(canvas_value, "getContext", &[&JsValue::from_str("webgl2"), &options])
}

/// The value the WebGL context `context` gives the constant `name`.
fn gl_constant(context: &JsValue, name: &str) -> Result<JsValue, JsValue> {
    js_sys::Reflect::get(context, &JsValue::from_str(name))
}

/// Compile `source` as a shader of the kind `context` names `kind`, failing with the compiler's log.
fn compile_gl_shader(context: &JsValue, kind: &str, source: &str) -> Result<JsValue, JsValue> {
    let shader = call_method(context, "createShader", &[&gl_constant(context, kind)?])?;
    invoke(context, "shaderSource", &[&shader, &JsValue::from_str(source)])?;
    invoke(context, "compileShader", &[&shader])?;
    let status = gl_constant(context, "COMPILE_STATUS")?;
    if call_method(context, "getShaderParameter", &[&shader, &status])?.is_truthy() {
        return Ok(shader);
    }
    let log = call_method(context, "getShaderInfoLog", &[&shader])?
        .as_string()
        .unwrap_or_default();
    Err(JsValue::from_str(&format!("{kind} did not compile: {log}")))
}

/// Link the probe vertex shader with the fragment shader `fragment_source`, failing with the linker's log.
fn link_probe_program(context: &JsValue, fragment_source: &str) -> Result<JsValue, JsValue> {
    let vertex = compile_gl_shader(context, "VERTEX_SHADER", et_gpu_probe::VERTEX_SHADER)?;
    let fragment = compile_gl_shader(context, "FRAGMENT_SHADER", fragment_source)?;
    let program = call_method(context, "createProgram", &[])?;
    invoke(context, "attachShader", &[&program, &vertex])?;
    invoke(context, "attachShader", &[&program, &fragment])?;
    invoke(context, "linkProgram", &[&program])?;
    let status = gl_constant(context, "LINK_STATUS")?;
    if call_method(context, "getProgramParameter", &[&program, &status])?.is_truthy() {
        return Ok(program);
    }
    let log = call_method(context, "getProgramInfoLog", &[&program])?
        .as_string()
        .unwrap_or_default();
    Err(JsValue::from_str(&format!("probe program did not link: {log}")))
}

/// Draw one probe's fragment shader over the context's single pixel and return the bits that pixel encodes.
fn run_fragment_probe(context: &JsValue, fragment_source: &str) -> Result<u32, JsValue> {
    let program = link_probe_program(context, fragment_source)?;
    invoke(context, "useProgram", &[&program])?;
    invoke(context, "disable", &[&gl_constant(context, "DITHER")?])?;
    let location = call_method(
        context,
        "getUniformLocation",
        &[&program, &JsValue::from_str("probeIn")],
    )?;
    let inputs: JsValue = js_sys::Float32Array::from(et_gpu_probe::INPUTS.as_slice()).into();
    invoke(context, "uniform1fv", &[&location, &inputs])?;

    let zero = JsValue::from_f64(0.0);
    let one = JsValue::from_f64(1.0);
    invoke(context, "drawArrays", &[&gl_constant(context, "POINTS")?, &zero, &one])?;
    let pixel = js_sys::Uint8Array::new_with_length(4);
    let pixel_value: &JsValue = pixel.as_ref();
    let format = gl_constant(context, "RGBA")?;
    let kind = gl_constant(context, "UNSIGNED_BYTE")?;
    invoke(
        context,
        "readPixels",
        &[&zero, &zero, &one, &one, &format, &kind, pixel_value],
    )?;
    // The shader wrote the lowest byte to red, so the pixel's four bytes are the result in little-endian order.
    Ok(js_sys::DataView::new(&pixel.buffer(), 0, 4).get_uint32_endian(0, true))
}

/// What the device did with the probe called `name` through `interface`, logging the reason where it refused.
fn probe_answer(interface: &str, name: &str, result: Result<u32, JsValue>) -> et_gpu_probe::Answer {
    match result {
        Ok(bits) => et_gpu_probe::Answer::Bits(bits),
        Err(error) => {
            let reason = describe_js_error(&error);
            info!("{interface} numeric probe {name} failed: {reason}");
            et_gpu_probe::Answer::Refused(reason)
        }
    }
}

/// What this device's GPU answered to the numeric probe set through one interface.
#[wasm_bindgen]
pub struct GpuNumericProbe {
    /// Whether the interface gave a device or context to put the probes to.
    success: bool,
    signature: et_gpu_probe::Signature,
}

#[wasm_bindgen]
impl GpuNumericProbe {
    /// Put each numeric probe to the WebGPU device as a compute shader of its own.
    ///
    /// Nothing a device does to a probe fails this call. A shader it will not compile, a validation error or a
    /// failed `mapAsync` is recorded as that probe's answer and the rest still run, because the probes exist to
    /// run on every GPU and a signature with gaps identifies a device better than none.
    #[wasm_bindgen(js_name = run)]
    pub async fn run() -> Result<Self, JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window"))?;
        let device = match request_gpu_device(&window).await {
            Ok(device) => device,
            Err(error) => {
                info!("GPU numeric probe: no device: {}", describe_js_error(&error));
                JsValue::NULL
            }
        };
        if device.is_null() || device.is_undefined() {
            return Ok(Self {
                signature: et_gpu_probe::Signature::new(et_gpu_probe::WEBGPU, Vec::new()),
                success: false,
            });
        }

        let mut observed = Vec::new();
        for (name, wgsl) in et_gpu_probe::shaders() {
            observed.push(probe_answer("WebGPU", name, run_probe_shader(&device, &wgsl).await));
        }
        Ok(Self {
            signature: et_gpu_probe::Signature::new(et_gpu_probe::WEBGPU, observed),
            success: true,
        })
    }

    /// Put each numeric probe to the GPU through WebGL 2, as a fragment shader drawn over one pixel.
    ///
    /// This reaches a GPU that WebGPU does not: a browser without WebGPU, or a page on an origin the browser
    /// will not give WebGPU to. As with [`Self::run`], nothing a device does to a probe fails this call.
    #[wasm_bindgen(js_name = runWebgl)]
    pub fn run_webgl() -> Result<Self, JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window"))?;
        let context = match create_probe_context(&window) {
            Ok(context) => context,
            Err(error) => {
                info!("WebGL numeric probe: no context: {}", describe_js_error(&error));
                JsValue::NULL
            }
        };
        if context.is_null() || context.is_undefined() {
            return Ok(Self {
                signature: et_gpu_probe::Signature::new(et_gpu_probe::WEBGL2, Vec::new()),
                success: false,
            });
        }

        let observed = et_gpu_probe::fragment_shaders()
            .into_iter()
            .map(|(name, source)| probe_answer("WebGL 2", name, run_fragment_probe(&context, &source)))
            .collect();
        Ok(Self {
            signature: et_gpu_probe::Signature::new(et_gpu_probe::WEBGL2, observed),
            success: true,
        })
    }

    /// How many probes the GPU answered.
    #[must_use]
    #[wasm_bindgen(js_name = completed)]
    pub fn completed(&self) -> usize {
        self.signature.completed()
    }

    /// How many probes the GPU answered with bits other than the correctly rounded ones.
    #[must_use]
    #[wasm_bindgen(js_name = deviations)]
    pub fn deviations(&self) -> usize {
        self.signature.deviations()
    }

    /// The GPU's answers as hex words joined by `-`: equal on two devices exactly when every probe agrees.
    #[must_use]
    #[wasm_bindgen(js_name = fingerprint)]
    pub fn fingerprint(&self) -> String {
        self.signature.fingerprint()
    }

    /// The known GPU that answers the probes exactly as this one did, or `unrecognised`.
    #[must_use]
    #[wasm_bindgen(js_name = gpuClass)]
    pub fn gpu_class(&self) -> String {
        self.signature.class().to_owned()
    }

    /// Which of two legal answers the GPU gave to each probe that has two.
    #[must_use]
    #[wasm_bindgen(js_name = traits)]
    pub fn traits(&self) -> String {
        self.signature.traits()
    }
}

/// Everything `probe` found, for the client event: enough to register this GPU from the hub's log alone.
fn numeric_report(probe: &GpuNumericProbe) -> serde_json::Value {
    json!({
        "success": probe.success,
        "class": probe.signature.class(),
        "answered": probe.signature.completed(),
        "deviations": probe.signature.deviations(),
        "traits": probe.signature.traits(),
        "fingerprint": probe.signature.fingerprint(),
        "probes": probe.signature.describe(),
    })
}

/// The outcome of `probe` in the few lines the module's status box has room for.
fn numeric_status(probe: &GpuNumericProbe) -> String {
    if !probe.success {
        return "skipped (interface unavailable)".to_string();
    }
    format!(
        "{} ({} answered, {} not correctly rounded)\n{}",
        probe.signature.class(),
        probe.signature.completed(),
        probe.signature.deviations(),
        probe.signature.fingerprint()
    )
}

/// Write what `probe` found through the interface called `interface` to the module's log.
fn log_numeric(interface: &str, probe: &GpuNumericProbe) {
    if !probe.success {
        log(&format!("{interface} numeric probe: skipped (interface unavailable)"));
        return;
    }
    log(&format!(
        "{interface} numeric probe: class={} answered={} deviations={}",
        probe.signature.class(),
        probe.signature.completed(),
        probe.signature.deviations()
    ));
    log(&format!(
        "{interface} numeric probe: traits: {}",
        probe.signature.traits()
    ));
    log(&format!(
        "{interface} numeric probe: fingerprint={}",
        probe.signature.fingerprint()
    ));
    for line in probe.signature.describe() {
        log(&format!("{interface} numeric probe: {line}"));
    }
}

/// What the browser says about itself and the page, which decides which interfaces a GPU can be reached through.
///
/// A page that is not a secure context is refused WebGPU whatever the GPU, so the two are reported together.
fn browser_environment(window: &web_sys::Window) -> Result<serde_json::Value, JsValue> {
    let navigator = window.navigator();
    let navigator: &JsValue = navigator.as_ref();
    let window_value: &JsValue = window.as_ref();
    Ok(json!({
        "user_agent": js_string_field(navigator, "userAgent"),
        "platform": js_string_field(navigator, "platform"),
        "secure_context": js_sys::Reflect::get(window_value, &JsValue::from_str("isSecureContext"))?.is_truthy(),
        "origin": js_string_field(&js_sys::Reflect::get(window_value, &JsValue::from_str("location"))?, "origin"),
    }))
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
    pub async fn detect() -> Result<Self, JsValue> {
        // A browser may blank the WebGPU adapter's device and description, which leaves no renderer to name.
        // WebGL's debug renderer string usually still names the GPU, so it fills that gap where it can.
        if let Some(info) = detect_webgpu_info().await? {
            if info.renderer != "unknown" {
                return Ok(info);
            }
            let Some(webgl) = detect_webgl_info()? else {
                return Ok(info);
            };
            return Ok(Self {
                architecture: info.architecture,
                description: webgl.description,
                renderer: webgl.renderer,
                source: "webgpu+webgl_debug_renderer_info".to_string(),
                vendor: if info.vendor == "unknown" {
                    webgl.vendor
                } else {
                    info.vendor
                },
            });
        }

        if let Some(info) = detect_webgl_info()? {
            return Ok(info);
        }

        Ok(Self {
            vendor: "unknown".to_string(),
            renderer: "unknown".to_string(),
            architecture: "unknown".to_string(),
            description: "No GPU details exposed by this browser".to_string(),
            source: "none".to_string(),
        })
    }

    #[must_use]
    pub fn vendor(&self) -> String {
        self.vendor.clone()
    }

    #[must_use]
    pub fn renderer(&self) -> String {
        self.renderer.clone()
    }

    #[must_use]
    pub fn architecture(&self) -> String {
        self.architecture.clone()
    }

    #[must_use]
    pub fn description(&self) -> String {
        self.description.clone()
    }

    #[must_use]
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

    let Some(request_adapter) = js_sys::Reflect::get(&gpu, &JsValue::from_str("requestAdapter"))
        .ok()
        .and_then(|value| value.dyn_into::<js_sys::Function>().ok())
    else {
        return Ok(None);
    };

    let adapter_promise = request_adapter.call0(&gpu)?.into_promise("requestAdapter")?;
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
            .into_promise("requestAdapterInfo")?;
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
    let canvas = create_canvas(&window)?;

    let context = canvas
        .get_context("webgl")?
        .or_else(|| canvas.get_context("webgl2").ok().flatten());

    let Some(context) = context else {
        return Ok(None);
    };

    let Some(get_extension) = js_sys::Reflect::get(&context, &JsValue::from_str("getExtension"))
        .ok()
        .and_then(|value| value.dyn_into::<js_sys::Function>().ok())
    else {
        return Ok(None);
    };

    let extension = get_extension.call1(&context, &JsValue::from_str("WEBGL_debug_renderer_info"))?;
    if extension.is_null() || extension.is_undefined() {
        return Ok(None);
    }

    let Some(get_parameter) = js_sys::Reflect::get(&context, &JsValue::from_str("getParameter"))
        .ok()
        .and_then(|value| value.dyn_into::<js_sys::Function>().ok())
    else {
        return Ok(None);
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
    et_web::ignore(tracing_wasm::try_set_as_global_default());
    info!("graphics-info module initialized");
}

#[must_use]
#[wasm_bindgen]
#[expect(
    clippy::missing_const_for_fn,
    reason = "wasm_bindgen rejects const fns; cannot be marked const"
)]
pub fn is_running() -> bool {
    false
}

#[expect(
    clippy::too_many_lines,
    reason = "linear scenario script: detect graphics support, probe WebGPU, run compute, emit JSON event"
)]
#[wasm_bindgen]
pub async fn run() -> Result<(), JsValue> {
    set_module_status("graphics-info: entered run()")?;
    log("entered run()");

    let outcome = async {
        let ws_url = websocket_url()?;
        let mut client = WsClient::new(WsClientConfig::new(ws_url));
        client.connect()?;
        wait_for_connected(&client).await?;
        log(&format!("websocket connected with agent_id={}", client.get_agent_id()));

        log("detecting graphics support");
        let support = GraphicsSupport::detect()?;
        log(&format!(
            "graphics support: webgl={} webgl2={} webgpu={} webnn={}",
            support.webgl_supported(),
            support.webgl2_supported(),
            support.webgpu_supported(),
            support.webnn_supported()
        ));

        log("probing WebGPU");
        let probe = WebGpuProbeResult::test().await?;
        log(&format!(
            "WebGPU probe: adapter={} device={}",
            probe.adapter_found(),
            probe.device_created()
        ));

        log("detecting GPU info");
        let gpu = GpuInfo::detect().await?;
        log(&format!(
            "GPU info: vendor={} renderer={} architecture={} source={}",
            gpu.vendor(),
            gpu.renderer(),
            gpu.architecture(),
            gpu.source()
        ));

        log("running GPU matrix multiply (4x4)");
        let compute = GpuComputeResult::run().await?;
        if compute.success() {
            let expected = 2.0_f32;
            if (compute.result_c00() - expected).abs() < 1e-4 {
                log("GPU compute: ok");
            } else {
                log(&format!(
                    "GPU compute: WRONG result C[0][0]={} (expected {expected})",
                    compute.result_c00()
                ));
            }
        } else {
            log("GPU compute: skipped (WebGPU unavailable)");
        }

        log("running numeric probe through WebGPU");
        let numeric = GpuNumericProbe::run().await?;
        log_numeric("WebGPU", &numeric);

        log("running numeric probe through WebGL 2");
        let numeric_webgl = GpuNumericProbe::run_webgl()?;
        log_numeric("WebGL 2", &numeric_webgl);

        let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
        let environment = browser_environment(&window)?;
        log(&format!("environment: {environment}"));

        client.send_client_event(
            et_org::served_npm_module_path!(),
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
                },
                "gpu_numeric_probe": numeric_report(&numeric),
                "webgl_numeric_probe": numeric_report(&numeric_webgl),
                "environment": environment,
            }),
        )?;

        set_module_status(&format!(
            concat!(
                "graphics-info: detected\nGPU: {}\nRenderer: {}\nWebGPU: {}\nCompute: {}\n",
                "WebGPU probe: {}\nWebGL 2 probe: {}"
            ),
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
            },
            numeric_status(&numeric),
            numeric_status(&numeric_webgl)
        ))?;

        client.disconnect();
        Ok(())
    }
    .await;

    if let Err(error) = &outcome {
        let message = describe_js_error(error);
        et_web::ignore(set_module_status(&format!("graphics-info: error\n{message}")));
        log(&format!("error: {message}"));
    }

    outcome
}

fn log(message: &str) {
    let line = format!("[graphics-info] {message}");
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
