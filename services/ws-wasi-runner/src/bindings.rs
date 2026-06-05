//! `wasmtime::component::bindgen!` output for the runner world.
//!
//! Lives in its own file so `no-inline-mod` can stay enforced on the
//! crate root: the macro generates a `mod`-shaped tree of types, which
//! would otherwise have to be wrapped in `pub mod bindings { ... }` at
//! the `lib.rs` top level.
#![expect(
    clippy::error_impl_error,
    clippy::exhaustive_enums,
    clippy::exhaustive_structs,
    clippy::impl_trait_in_params,
    clippy::integer_division_remainder_used,
    clippy::missing_asserts_for_indexing,
    reason = "wasmtime::component::bindgen! generates the API surface from WIT; we don't control its lints"
)]

//! wasmtime-bindgen output for the `runner` world declared in
//! `generated/specs/wit/world.wit`. Every WIT type defined in the world or
//! its dep packages is reachable through `crate::bindings::<namespace>`.
//!
//! The `with:` map points wasi-webgpu's resource types at our wgpu-backed
//! payload structs (defined in `host::wasi_webgpu`); without it, the
//! bindgen-generated marker structs would be opaque and the `resource_table`
//! couldn't carry real wgpu objects.
wasmtime::component::bindgen!({
    path: "../../generated/specs/wit",
    world: "runner",
    imports: { default: async },
    exports: { default: async },
    with: {
        "wasi:keyvalue/store.bucket": super::host::wasi_keyvalue::Bucket,
        "wasi:webgpu/webgpu.gpu": super::host::wasi_webgpu::Gpu,
        "wasi:webgpu/webgpu.gpu-adapter": super::host::wasi_webgpu::GpuAdapter,
        "wasi:webgpu/webgpu.gpu-adapter-info": super::host::wasi_webgpu::GpuAdapterInfo,
        "wasi:webgpu/webgpu.gpu-supported-features": super::host::wasi_webgpu::GpuSupportedFeatures,
        "wasi:webgpu/webgpu.gpu-supported-limits": super::host::wasi_webgpu::GpuSupportedLimits,
        "wasi:webgpu/webgpu.gpu-device": super::host::wasi_webgpu::GpuDevice,
        "wasi:webgpu/webgpu.gpu-queue": super::host::wasi_webgpu::GpuQueue,
        "wasi:webgpu/webgpu.gpu-buffer": super::host::wasi_webgpu::GpuBuffer,
        "wasi:webgpu/webgpu.gpu-buffer-usage": super::host::wasi_webgpu::GpuBufferUsage,
        "wasi:webgpu/webgpu.gpu-map-mode": super::host::wasi_webgpu::GpuMapMode,
        "wasi:webgpu/webgpu.gpu-shader-stage": super::host::wasi_webgpu::GpuShaderStage,
        "wasi:webgpu/webgpu.gpu-bind-group-layout": super::host::wasi_webgpu::GpuBindGroupLayout,
        "wasi:webgpu/webgpu.gpu-bind-group": super::host::wasi_webgpu::GpuBindGroup,
        "wasi:webgpu/webgpu.gpu-pipeline-layout": super::host::wasi_webgpu::GpuPipelineLayout,
        "wasi:webgpu/webgpu.gpu-shader-module": super::host::wasi_webgpu::GpuShaderModule,
        "wasi:webgpu/webgpu.gpu-compute-pipeline": super::host::wasi_webgpu::GpuComputePipeline,
        "wasi:webgpu/webgpu.gpu-command-encoder": super::host::wasi_webgpu::GpuCommandEncoder,
        "wasi:webgpu/webgpu.gpu-compute-pass-encoder": super::host::wasi_webgpu::GpuComputePassEncoder,
        "wasi:webgpu/webgpu.gpu-command-buffer": super::host::wasi_webgpu::GpuCommandBuffer,
        "wasi:webgpu/webgpu.record-option-gpu-size64": super::host::wasi_webgpu::RecordOptionGpuSize64,
        "wasi:webgpu/webgpu.record-gpu-pipeline-constant-value":
            super::host::wasi_webgpu::RecordGpuPipelineConstantValue,
    },
});
