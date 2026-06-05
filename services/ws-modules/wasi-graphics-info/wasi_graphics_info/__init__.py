"""WASI port of `graphics-info`, with both wasi-webgpu compute and a wasi-nn ML demo.

The module exercises two standardised WASI interfaces on the host:

* `wasi:webgpu/webgpu` — a trimmed subset of WebAssembly/wasi-gfx (see
  `wit/deps/wasi-webgpu/`). The guest builds a real GPU pipeline (adapter,
  device, buffers, shader, bind group, compute pass) and runs a 4x4 matmul.
* `wasi:nn/{graph, tensor, inference}` — the same WIT surface wasmCloud /
  Spin / Fermyon production workloads use, backed on our host by
  `wasmtime-wasi-nn` + ONNX Runtime. We load `mnist-12.onnx` and run a
  single forward pass.

Workflow per run():
  1. set up a webgpu adapter/device + report graphics info
  2. run a 4x4 matmul compute pass and verify the (0,0) result
  3. load `mnist-12.onnx` (bundled, ~26KB) via `wasi:nn/graph.load`
  4. run inference on a fixed 28x28 all-zeros input
  5. argmax -> predicted MNIST digit class
  6. verify against EXPECTED_MNIST_CLASS, send the result over ws
"""

import array
import json
import struct

from componentize_py_types import Err
from wit_world.exports.entry import EntryError_Runtime, EntryError_Ws
from wit_world.imports import graph as nn_graph
from wit_world.imports import (
    logging,
    messages,
    monotonic_clock,
    poll,
    store,
    webgpu,
    ws,
)
from wit_world.imports.graph import ExecutionTarget, GraphEncoding
from wit_world.imports.logging import Level
from wit_world.imports.tensor import Tensor, TensorType
from wit_world.imports.webgpu import (
    GpuBindGroupDescriptor,
    GpuBindGroupEntry,
    GpuBindGroupLayoutDescriptor,
    GpuBindGroupLayoutEntry,
    GpuBindingResource_GpuBufferBinding,
    GpuBufferBinding,
    GpuBufferBindingLayout,
    GpuBufferBindingType,
    GpuBufferDescriptor,
    GpuBufferUsage,
    GpuComputePipelineDescriptor,
    GpuLayoutMode_Specific,
    GpuMapMode,
    GpuPipelineLayoutDescriptor,
    GpuProgrammableStage,
    GpuShaderModuleDescriptor,
    GpuShaderStage,
)
from wit_world.imports.ws import (
    WsError_AlreadyConnected,
    WsError_Decode,
    WsError_NotConnected,
    WsError_Transport,
)

_WS_ERROR_VARIANTS = (
    WsError_Transport,
    WsError_Decode,
    WsError_NotConnected,
    WsError_AlreadyConnected,
)

# An all-zero 28x28 input is the simplest reproducible MNIST query - bytes
# don't drift across rebuilds and we don't have to ship a real digit image.
# Whatever class ONNX Runtime returns for this fixed input is what we
# hard-code as expected. mnist-12.onnx happens to confidently predict class
# 5 for an all-black image (its bias toward "5" on blank-ish inputs is a
# well-known property).
MNIST_INPUT_SHAPE = [1, 1, 28, 28]
EXPECTED_MNIST_CLASS = 5

# mnist-12.onnx I/O names - established by the model file, not configurable.
MNIST_INPUT_NAME = "Input3"
MNIST_OUTPUT_NAME = "Plus214_Output_0"

# Identity * (2*I) matmul. Result C[0][0] = 2.0, C[1][1] = 2.0, ...
MAT_A = [
    1.0,
    0.0,
    0.0,
    0.0,
    0.0,
    1.0,
    0.0,
    0.0,
    0.0,
    0.0,
    1.0,
    0.0,
    0.0,
    0.0,
    0.0,
    1.0,
]
MAT_B = [
    2.0,
    0.0,
    0.0,
    0.0,
    0.0,
    2.0,
    0.0,
    0.0,
    0.0,
    0.0,
    2.0,
    0.0,
    0.0,
    0.0,
    0.0,
    2.0,
]
MATRIX_BYTES = 16 * 4  # 16 f32 = 64 bytes
EXPECTED_C00 = 2.0

WGSL = """
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
"""


LOG_CONTEXT = "wasi-graphics-info"


def _log(message: str) -> None:
    # All current call sites are informational. Drop down to `logging.log` with
    # an explicit level if a warn/error path needs it.
    logging.log(Level.INFO, LOG_CONTEXT, message)


def _send_event(category: str, kind: str, body: dict) -> None:
    ws.send(
        messages.ClientMessage_ClientEvent(
            messages.ClientEventPayload(
                capability=category,
                action=kind,
                details=json.dumps(body),
            )
        )
    )


def _now_ms() -> int:
    # `monotonic_clock.now()` returns nanoseconds from an arbitrary epoch.
    # Only used here for elapsed-time deltas, so the epoch doesn't matter.
    return monotonic_clock.now() // 1_000_000


def _sleep_ms(ms: int) -> None:
    # The standard WASI sleep: subscribe a duration pollable, then block on
    # it via wasi:io/poll. Equivalent to `clock.sleep-ms` in the old custom
    # interface, but uses interfaces every WASI Preview 2 runtime provides.
    pollable = monotonic_clock.subscribe_duration(ms * 1_000_000)
    poll.poll([pollable])


def _wait_for_connected(timeout_ms: int = 2000) -> bool:
    elapsed = 0
    step = 50
    while elapsed < timeout_ms:
        if ws.get_state() == ws.State.CONNECTED:
            return True
        _sleep_ms(step)
        elapsed += step
    return False


def _zero_input_bytes() -> bytes:
    # 28x28 = 784 float32 zeros, little-endian (wasi-nn's tensor data layout
    # follows host native; both wasmtime targets and CPython use little-endian
    # on every platform we ship to).
    return array.array("f", [0.0] * (28 * 28)).tobytes()


def _matrix_bytes(values: list) -> bytes:
    return struct.pack(f"<{len(values)}f", *values)


def _entry(binding: int, read_only: bool) -> GpuBindGroupLayoutEntry:
    """One COMPUTE-visible storage-buffer bind-group-layout entry. Bindings 0
    and 1 are read-only (matA / matB), binding 2 is read-write (matC). This
    has to match the WGSL `var<storage, read>` / `var<storage, read_write>`
    qualifiers exactly or wgpu's create-compute-pipeline validation rejects."""
    return GpuBindGroupLayoutEntry(
        binding=binding,
        visibility=GpuShaderStage.compute(),
        buffer=GpuBufferBindingLayout(
            type=GpuBufferBindingType.READ_ONLY_STORAGE if read_only else GpuBufferBindingType.STORAGE,
            has_dynamic_offset=False,
            min_binding_size=None,
        ),
    )


def _bind_entry(binding: int, buffer) -> GpuBindGroupEntry:
    return GpuBindGroupEntry(
        binding=binding,
        resource=GpuBindingResource_GpuBufferBinding(
            value=GpuBufferBinding(buffer=buffer, offset=0, size=MATRIX_BYTES),
        ),
    )


def _run_matmul() -> dict:
    """Build a full wasi-webgpu compute pipeline and run the 4x4 matmul.

    Returns the wire-format dict for `gpu_compute`. Raises `RuntimeError`
    if the readback's first element doesn't match `EXPECTED_C00`.
    """
    _log("wasi-webgpu: requesting adapter")
    gpu = webgpu.get_gpu()
    adapter = gpu.request_adapter(None)
    if adapter is None:
        raise RuntimeError("wasi-webgpu: no GPU adapter available")

    info = adapter.info()
    gpu_info = {
        "vendor": info.vendor(),
        "renderer": info.device(),
        "architecture": info.architecture(),
        "description": info.description(),
        "source": "wasi-webgpu",
    }
    _log(
        f"wasi-webgpu adapter: vendor={gpu_info['vendor']} renderer={gpu_info['renderer']}"
        f" architecture={gpu_info['architecture']}"
    )

    device = adapter.request_device(None)
    queue = device.queue()

    started = _now_ms()

    storage_init_usage = GpuBufferUsage.storage() | GpuBufferUsage.copy_dst()
    storage_out_usage = GpuBufferUsage.storage() | GpuBufferUsage.copy_src()
    readback_usage = GpuBufferUsage.map_read() | GpuBufferUsage.copy_dst()

    buf_a = device.create_buffer(
        GpuBufferDescriptor(
            size=MATRIX_BYTES,
            usage=storage_init_usage,
            mapped_at_creation=False,
            label="matA",
        )
    )
    buf_b = device.create_buffer(
        GpuBufferDescriptor(
            size=MATRIX_BYTES,
            usage=storage_init_usage,
            mapped_at_creation=False,
            label="matB",
        )
    )
    buf_c = device.create_buffer(
        GpuBufferDescriptor(
            size=MATRIX_BYTES,
            usage=storage_out_usage,
            mapped_at_creation=False,
            label="matC",
        )
    )
    buf_readback = device.create_buffer(
        GpuBufferDescriptor(
            size=MATRIX_BYTES,
            usage=readback_usage,
            mapped_at_creation=False,
            label="readback",
        )
    )

    queue.write_buffer_with_copy(buf_a, 0, _matrix_bytes(MAT_A), None, None)
    queue.write_buffer_with_copy(buf_b, 0, _matrix_bytes(MAT_B), None, None)

    shader = device.create_shader_module(
        GpuShaderModuleDescriptor(code=WGSL, compilation_hints=None, label="matmul-4x4")
    )

    bgl = device.create_bind_group_layout(
        GpuBindGroupLayoutDescriptor(
            entries=[
                _entry(0, read_only=True),
                _entry(1, read_only=True),
                _entry(2, read_only=False),
            ],
            label="matmul-bgl",
        )
    )
    pl = device.create_pipeline_layout(GpuPipelineLayoutDescriptor(bind_group_layouts=[bgl], label="matmul-pl"))

    pipeline = device.create_compute_pipeline(
        GpuComputePipelineDescriptor(
            compute=GpuProgrammableStage(module=shader, entry_point="main", constants=None),
            layout=GpuLayoutMode_Specific(value=pl),
            label="matmul-pipeline",
        )
    )

    bind_group = device.create_bind_group(
        GpuBindGroupDescriptor(
            layout=bgl,
            entries=[
                _bind_entry(0, buf_a),
                _bind_entry(1, buf_b),
                _bind_entry(2, buf_c),
            ],
            label="matmul-bg",
        )
    )

    encoder = device.create_command_encoder(None)
    pass_ = encoder.begin_compute_pass(None)
    pass_.set_pipeline(pipeline)
    pass_.set_bind_group(0, bind_group, None, None, None)
    # The shader uses @workgroup_size(4,4), so one workgroup covers all 16 cells.
    pass_.dispatch_workgroups(1, 1, 1)
    pass_.end()

    encoder.copy_buffer_to_buffer(buf_c, 0, buf_readback, 0, MATRIX_BYTES)
    command_buffer = encoder.finish(None)
    queue.submit([command_buffer])

    buf_readback.map_async(GpuMapMode.read(), 0, MATRIX_BYTES)
    data = buf_readback.get_mapped_range_get_with_copy(0, MATRIX_BYTES)
    buf_readback.unmap()
    elapsed_ms = float(_now_ms() - started)

    result_c00 = struct.unpack("<f", bytes(data[:4]))[0]
    if abs(result_c00 - EXPECTED_C00) > 1e-4:
        raise RuntimeError(f"wasi-webgpu: matmul produced C[0][0]={result_c00}, expected {EXPECTED_C00}")
    _log(f"wasi-webgpu matmul: C[0][0]={result_c00:.4f} in {elapsed_ms:.2f}ms")

    return {
        "gpu_info": gpu_info,
        "gpu_compute": {
            "success": True,
            "elapsed_ms": elapsed_ms,
            "result_c00": float(result_c00),
        },
        "webgpu_probe": {"adapter_found": True, "device_created": True},
    }


def _mnist_inference() -> dict:
    """Load mnist-12.onnx and run a single forward pass via wasi-nn.

    Returns a dict suitable for inclusion in the `client_event` payload, or
    raises a `RuntimeError` if the prediction doesn't match expectation.
    """
    _log("loading mnist-12.onnx")
    # The model file is a sibling static asset, served from pkg/ by
    # et-modules-service. We treat it as a read-only wasi:keyvalue bucket
    # backed by the module's static-asset directory (`/modules/<name>/`).
    module_assets = store.open("modules/et-ws-wasi-graphics-info")
    model_value = module_assets.get("mnist-12.onnx")
    if model_value is None:
        raise RuntimeError("mnist-12.onnx not found in modules bucket")
    model_bytes = bytes(model_value)
    _log(f"model loaded: {len(model_bytes)} bytes")

    # wasi-nn's `graph.load` takes a list of builders - for ONNX it's just
    # the single model file. ExecutionTarget.GPU is a hint; the host backend
    # (ORT) decides what hardware to dispatch to.
    g = nn_graph.load([model_bytes], GraphEncoding.ONNX, ExecutionTarget.GPU)
    _log("graph loaded")

    ctx = g.init_execution_context()
    _log("execution context ready")

    input_tensor = Tensor(MNIST_INPUT_SHAPE, TensorType.FP32, _zero_input_bytes())

    _log("running inference")
    started = _now_ms()
    outputs = ctx.compute([(MNIST_INPUT_NAME, input_tensor)])
    elapsed_ms = _now_ms() - started
    _log(f"inference complete in {elapsed_ms}ms")

    if not outputs:
        raise RuntimeError("wasi-nn returned no outputs")

    out_name, out_tensor = outputs[0]
    if out_name != MNIST_OUTPUT_NAME:
        _log(f"warning: output name {out_name!r} differs from expected {MNIST_OUTPUT_NAME!r}")

    raw = out_tensor.data()
    arr = array.array("f")
    arr.frombytes(raw)
    logits = list(arr)

    if len(logits) != 10:
        raise RuntimeError(f"expected 10 MNIST logits, got {len(logits)}")

    predicted = max(range(10), key=lambda i: logits[i])
    _log(f"predicted class: {predicted}, logits: {[round(v, 3) for v in logits]}")

    if predicted != EXPECTED_MNIST_CLASS:
        raise RuntimeError(f"MNIST verification FAILED: predicted {predicted}, expected {EXPECTED_MNIST_CLASS}")
    _log("MNIST verification: ok")

    return {
        "framework": "wasi-nn (onnxruntime)",
        "model": "mnist-12.onnx",
        "input_shape": MNIST_INPUT_SHAPE,
        "predicted_class": predicted,
        "expected_class": EXPECTED_MNIST_CLASS,
        "elapsed_ms": elapsed_ms,
        "logits": [round(float(v), 4) for v in logits],
    }


_STORE_ERROR_VARIANTS = (
    store.Error_NoSuchStore,
    store.Error_AccessDenied,
    store.Error_Other,
)


class Entry:
    """Implements the `entry` interface exported by the world.

    WIT signature is now `run: func() -> result<_, entry-error>` where
    `entry-error` is a variant `ws(ws-error) | runtime(string)`.
    componentize-py renders the success path as `-> None` and failures as
    `raise Err(<variant case>)`. Workflow code lives in `_run_workflow`;
    this wrapper lifts a raw `ws-error` payload bubbling up from any
    `ws.*` call into `EntryError_Ws(...)`, and tags every other
    exception (`store.Error` variants, string messages we raise
    ourselves, etc.) as `EntryError_Runtime(...)`.
    """

    def run(self) -> None:
        try:
            _run_workflow()
        except Err as exc:
            value = exc.value
            if isinstance(value, _WS_ERROR_VARIANTS):
                raise Err(EntryError_Ws(value)) from exc
            if isinstance(value, _STORE_ERROR_VARIANTS):
                raise Err(EntryError_Runtime(f"store error: {value}")) from exc
            raise Err(EntryError_Runtime(str(value))) from exc


def _run_workflow() -> None:
    _log("entered run()")

    ws.connect()
    if not _wait_for_connected():
        raise Err(EntryError_Runtime("websocket did not reach connected state"))

    agent_id = ws.agent_id()
    _log(f"websocket connected with agent_id={agent_id}")

    gpu_block = _run_matmul()
    # No browser-level detection in WASI; report the wasi-webgpu fact as
    # the only WebGPU signal and the legacy WebGL / WebNN flags as False.
    support = {"webgl": False, "webgl2": False, "webgpu": True, "webnn": False}

    mnist_result = _mnist_inference()

    _send_event(
        "graphics",
        "info_detected",
        {
            "support": support,
            "webgpu_probe": gpu_block["webgpu_probe"],
            "gpu": gpu_block["gpu_info"],
            "gpu_compute": gpu_block["gpu_compute"],
            "mnist_inference": mnist_result,
        },
    )

    ws.disconnect()
