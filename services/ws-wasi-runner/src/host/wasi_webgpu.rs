//! Host impl of the trimmed `wasi:webgpu` surface (see
//! `wit/deps/wasi-webgpu/webgpu.wit` for the subset). Only the matmul
//! critical path is wired through to real `wgpu` calls; every other kept
//! method traps with `unimplemented!`, so guests that stray off the path
//! fail loudly rather than silently misbehaving.
//!
//! Resource handles are stored in `HostState.resource_table` via wasmtime's
//! `Resource<T>` machinery; the mapping from WIT resource names to the
//! payload types declared in this file is set up by the `with:` block of
//! the `wasmtime::component::bindgen!` invocation in `lib.rs`.
//!
//! Compute passes don't get their own live `wgpu::ComputePass` — that type
//! borrows the encoder mutably, which can't sit in a resource table. We
//! buffer pass commands on the encoder resource and replay them inside
//! `end()`, so the real `ComputePass` lives only for the duration of one
//! synchronous block.
#![expect(
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::clone_on_ref_ptr,
    clippy::default_trait_access,
    clippy::doc_markdown,
    clippy::exhaustive_enums,
    clippy::exhaustive_structs,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::min_ident_chars,
    clippy::needless_pass_by_ref_mut,
    clippy::redundant_clone,
    clippy::renamed_function_params,
    clippy::single_call_fn,
    clippy::too_long_first_doc_paragraph,
    clippy::unimplemented,
    let_underscore_drop,
    unused_results,
    reason = "trimmed wasi-webgpu host: only matmul path is wired, others trap; to be replaced by upstream"
)]

use std::collections::BTreeMap;
use std::sync::Arc;

use wasmtime::component::Resource;

use crate::HostState;
use crate::bindings::wasi::webgpu::webgpu::{
    self as wg, CreatePipelineError, CreatePipelineErrorKind, GetMappedRangeError, GetMappedRangeErrorKind,
    GpuBindGroupDescriptor, GpuBindGroupLayoutDescriptor, GpuBufferDescriptor, GpuBufferMapState,
    GpuComputePassDescriptor, GpuComputePipelineDescriptor, GpuLayoutMode, GpuPipelineErrorReason,
    GpuPipelineLayoutDescriptor, GpuRequestAdapterOptions, GpuShaderModuleDescriptor, Host, HostGpu, HostGpuAdapter,
    HostGpuAdapterInfo, HostGpuBindGroup, HostGpuBindGroupLayout, HostGpuBuffer, HostGpuBufferUsage,
    HostGpuCommandBuffer, HostGpuCommandEncoder, HostGpuComputePassEncoder, HostGpuComputePipeline, HostGpuDevice,
    HostGpuMapMode, HostGpuPipelineLayout, HostGpuQueue, HostGpuShaderModule, HostGpuShaderStage,
    HostGpuSupportedFeatures, HostGpuSupportedLimits, HostRecordGpuPipelineConstantValue, HostRecordOptionGpuSize64,
    MapAsyncError, MapAsyncErrorKind, RequestDeviceError, SetBindGroupError, SetBindGroupErrorKind, UnmapError,
    UnmapErrorKind, WriteBufferError, WriteBufferErrorKind,
};
use crate::host::{RequestDeviceErrExt as _, WitErrExt as _};

/// wgpu buffer-usage flags as the host wire-format. The WIT-side
/// `gpu-buffer-usage.STORAGE()` style accessors return these constants and
/// the guest ORs them into `gpu-buffer-descriptor.usage`. Matches the
/// WebGPU spec values so we can hand them directly to `wgpu::BufferUsages`.
struct Usage;
impl Usage {
    const MAP_READ: u32 = 0x0001;
    const MAP_WRITE: u32 = 0x0002;
    const COPY_SRC: u32 = 0x0004;
    const COPY_DST: u32 = 0x0008;
    const INDEX: u32 = 0x0010;
    const VERTEX: u32 = 0x0020;
    const UNIFORM: u32 = 0x0040;
    const STORAGE: u32 = 0x0080;
    const INDIRECT: u32 = 0x0100;
    const QUERY_RESOLVE: u32 = 0x0200;
}

/// gpu-map-mode flag bits (WebGPU spec values).
struct MapMode;
impl MapMode {
    const READ: u32 = 0x0001;
    const WRITE: u32 = 0x0002;
}

/// gpu-shader-stage flag bits (WebGPU spec values).
struct ShaderStage;
impl ShaderStage {
    const VERTEX: u32 = 0x1;
    const FRAGMENT: u32 = 0x2;
    const COMPUTE: u32 = 0x4;
}

/// Top-level handle: no per-instance state — `request-adapter` constructs a
/// fresh `wgpu::Instance` each call rather than sharing one across guests.
pub struct Gpu;

pub struct GpuAdapter {
    pub adapter: Arc<wgpu::Adapter>,
}

pub struct GpuAdapterInfo {
    pub info: wgpu::AdapterInfo,
}

pub struct GpuSupportedFeatures;
pub struct GpuSupportedLimits;

pub struct GpuDevice {
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
}

pub struct GpuQueue {
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
}

pub struct GpuBuffer {
    pub buffer: Arc<wgpu::Buffer>,
    pub device: Arc<wgpu::Device>,
    pub size: u64,
    pub usage: u32,
    pub map_state: GpuBufferMapState,
}

pub struct GpuBufferUsage;
pub struct GpuMapMode;
pub struct GpuShaderStage;

pub struct GpuBindGroupLayout {
    pub layout: Arc<wgpu::BindGroupLayout>,
}

pub struct GpuBindGroup {
    pub group: Arc<wgpu::BindGroup>,
}

pub struct GpuPipelineLayout {
    pub layout: Arc<wgpu::PipelineLayout>,
}

pub struct GpuShaderModule {
    pub module: Arc<wgpu::ShaderModule>,
}

pub struct GpuComputePipeline {
    pub pipeline: Arc<wgpu::ComputePipeline>,
}

/// Buffered command for a not-yet-replayed compute pass. We clone the wgpu
/// objects (all `Clone` thanks to wgpu's internal Arc-ing), so the pass can
/// be replayed inside `compute-pass.end()` without holding live borrows
/// across resource-table calls.
pub enum PassCommand {
    SetPipeline(Arc<wgpu::ComputePipeline>),
    SetBindGroup {
        index: u32,
        group: Arc<wgpu::BindGroup>,
        offsets: Vec<u32>,
    },
    DispatchWorkgroups(u32, u32, u32),
}

pub struct GpuCommandEncoder {
    pub device: Arc<wgpu::Device>,
    pub encoder: Option<wgpu::CommandEncoder>,
    /// Set to `Some(vec)` while a compute pass is being recorded; replayed
    /// against a freshly-opened `wgpu::ComputePass` when the pass's
    /// `end()` is called, then taken back out.
    pub pending_pass: Option<Vec<PassCommand>>,
}

/// Compute-pass resource: a tag pointing back at its parent encoder so
/// command-recording methods can find the pending command list. We use the
/// resource `rep()` (a u32 identity) rather than holding a `Resource<...>`
/// since the parent encoder might be looked up in either get/get_mut form.
pub struct GpuComputePassEncoder {
    pub encoder_rep: u32,
    pub ended: bool,
}

pub struct GpuCommandBuffer {
    pub buffer: Option<wgpu::CommandBuffer>,
}

/// `record-option-gpu-size64` and `record-gpu-pipeline-constant-value` are
/// WIT-side associative maps the guest can build up to pass to
/// `gpu-device-descriptor` / `gpu-programmable-stage`. We never actually
/// consume them in the matmul path, but they need at least a working ctor
/// so the WIT round-trip doesn't trap.
pub struct RecordOptionGpuSize64 {
    pub map: BTreeMap<String, Option<u64>>,
}

pub struct RecordGpuPipelineConstantValue {
    pub map: BTreeMap<String, f64>,
}

/// Build a fresh adapter from a new instance. `request-adapter` could be
/// called multiple times by a guest; each call gets its own adapter handle,
/// even if they all back onto the same underlying GPU.
async fn request_adapter_inner(_options: Option<GpuRequestAdapterOptions>) -> Option<wgpu::Adapter> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::PRIMARY,
        flags: wgpu::InstanceFlags::default(),
        backend_options: wgpu::BackendOptions::default(),
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        display: None,
    });
    instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        })
        .await
        .ok()
}

fn buffer_usage_from_flags(flags: u32) -> wgpu::BufferUsages {
    let mut out = wgpu::BufferUsages::empty();
    if flags & Usage::MAP_READ != 0 {
        out |= wgpu::BufferUsages::MAP_READ;
    }
    if flags & Usage::MAP_WRITE != 0 {
        out |= wgpu::BufferUsages::MAP_WRITE;
    }
    if flags & Usage::COPY_SRC != 0 {
        out |= wgpu::BufferUsages::COPY_SRC;
    }
    if flags & Usage::COPY_DST != 0 {
        out |= wgpu::BufferUsages::COPY_DST;
    }
    if flags & Usage::INDEX != 0 {
        out |= wgpu::BufferUsages::INDEX;
    }
    if flags & Usage::VERTEX != 0 {
        out |= wgpu::BufferUsages::VERTEX;
    }
    if flags & Usage::UNIFORM != 0 {
        out |= wgpu::BufferUsages::UNIFORM;
    }
    if flags & Usage::STORAGE != 0 {
        out |= wgpu::BufferUsages::STORAGE;
    }
    if flags & Usage::INDIRECT != 0 {
        out |= wgpu::BufferUsages::INDIRECT;
    }
    if flags & Usage::QUERY_RESOLVE != 0 {
        out |= wgpu::BufferUsages::QUERY_RESOLVE;
    }
    out
}

fn shader_stages_from_flags(flags: u32) -> wgpu::ShaderStages {
    let mut out = wgpu::ShaderStages::empty();
    if flags & ShaderStage::VERTEX != 0 {
        out |= wgpu::ShaderStages::VERTEX;
    }
    if flags & ShaderStage::FRAGMENT != 0 {
        out |= wgpu::ShaderStages::FRAGMENT;
    }
    if flags & ShaderStage::COMPUTE != 0 {
        out |= wgpu::ShaderStages::COMPUTE;
    }
    out
}

fn buffer_binding_type(t: Option<wg::GpuBufferBindingType>) -> wgpu::BufferBindingType {
    match t.unwrap_or(wg::GpuBufferBindingType::Storage) {
        wg::GpuBufferBindingType::Uniform => wgpu::BufferBindingType::Uniform,
        wg::GpuBufferBindingType::Storage => wgpu::BufferBindingType::Storage { read_only: false },
        wg::GpuBufferBindingType::ReadOnlyStorage => wgpu::BufferBindingType::Storage { read_only: true },
    }
}

impl Host for HostState {
    async fn get_gpu(&mut self) -> Resource<Gpu> {
        self.resource_table.push(Gpu).expect("resource table push")
    }
}

impl HostGpu for HostState {
    async fn request_adapter(
        &mut self,
        _gpu: Resource<Gpu>,
        options: Option<GpuRequestAdapterOptions>,
    ) -> Option<Resource<GpuAdapter>> {
        let adapter = request_adapter_inner(options).await?;
        let res = self
            .resource_table
            .push(GpuAdapter {
                adapter: Arc::new(adapter),
            })
            .expect("resource table push");
        Some(res)
    }

    async fn drop(&mut self, rep: Resource<Gpu>) -> wasmtime::Result<()> {
        self.resource_table.delete(rep)?;
        Ok(())
    }
}

impl HostGpuAdapter for HostState {
    async fn info(&mut self, rep: Resource<GpuAdapter>) -> Resource<GpuAdapterInfo> {
        let adapter = self.resource_table.get(&rep).expect("adapter handle");
        let info = adapter.adapter.get_info();
        self.resource_table
            .push(GpuAdapterInfo { info })
            .expect("resource table push")
    }

    async fn request_device(
        &mut self,
        rep: Resource<GpuAdapter>,
        _descriptor: Option<wg::GpuDeviceDescriptor>,
    ) -> Result<Resource<GpuDevice>, RequestDeviceError> {
        let adapter = self.resource_table.get(&rep).expect("adapter handle").adapter.clone();
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("wasi-webgpu host device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults(),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
                experimental_features: wgpu::ExperimentalFeatures::default(),
            })
            .await
            .request_device_err()?;
        let device = Arc::new(device);
        let queue = Arc::new(queue);
        let res = self
            .resource_table
            .push(GpuDevice {
                device: device.clone(),
                queue: queue.clone(),
            })
            .expect("resource table push");
        Ok(res)
    }

    async fn features(&mut self, _rep: Resource<GpuAdapter>) -> Resource<GpuSupportedFeatures> {
        unimplemented!("wasi-webgpu: adapter.features not implemented in matmul subset")
    }

    async fn limits(&mut self, _rep: Resource<GpuAdapter>) -> Resource<GpuSupportedLimits> {
        unimplemented!("wasi-webgpu: adapter.limits not implemented in matmul subset")
    }

    async fn is_fallback_adapter(&mut self, _rep: Resource<GpuAdapter>) -> bool {
        unimplemented!("wasi-webgpu: adapter.is-fallback-adapter not implemented in matmul subset")
    }

    async fn drop(&mut self, rep: Resource<GpuAdapter>) -> wasmtime::Result<()> {
        self.resource_table.delete(rep)?;
        Ok(())
    }
}

impl HostGpuAdapterInfo for HostState {
    async fn vendor(&mut self, rep: Resource<GpuAdapterInfo>) -> String {
        let info = &self.resource_table.get(&rep).expect("info handle").info;
        vendor_name(info.vendor)
    }

    async fn architecture(&mut self, rep: Resource<GpuAdapterInfo>) -> String {
        let info = &self.resource_table.get(&rep).expect("info handle").info;
        format!("{:?}", info.device_type).to_lowercase()
    }

    async fn device(&mut self, rep: Resource<GpuAdapterInfo>) -> String {
        let info = &self.resource_table.get(&rep).expect("info handle").info;
        info.name.clone()
    }

    async fn description(&mut self, rep: Resource<GpuAdapterInfo>) -> String {
        let info = &self.resource_table.get(&rep).expect("info handle").info;
        format!("{} ({:?}, {})", info.name, info.backend, info.driver_info)
    }

    async fn subgroup_min_size(&mut self, _rep: Resource<GpuAdapterInfo>) -> u32 {
        unimplemented!("wasi-webgpu: adapter-info.subgroup-min-size not implemented")
    }

    async fn subgroup_max_size(&mut self, _rep: Resource<GpuAdapterInfo>) -> u32 {
        unimplemented!("wasi-webgpu: adapter-info.subgroup-max-size not implemented")
    }

    async fn drop(&mut self, rep: Resource<GpuAdapterInfo>) -> wasmtime::Result<()> {
        self.resource_table.delete(rep)?;
        Ok(())
    }
}

/// Map a PCI vendor id (as wgpu reports it) to a human-friendly name. Falls
/// back to the raw hex so unknown vendors still surface usefully.
fn vendor_name(id: u32) -> String {
    match id {
        0x1002 => "AMD".into(),
        0x10de => "NVIDIA".into(),
        0x8086 => "Intel".into(),
        0x106b => "Apple".into(),
        0x13b5 => "ARM".into(),
        0x5143 => "Qualcomm".into(),
        0 => "unknown".into(),
        other => format!("0x{other:04x}"),
    }
}

impl HostGpuSupportedFeatures for HostState {
    async fn has(&mut self, _rep: Resource<GpuSupportedFeatures>, _value: String) -> bool {
        unimplemented!("wasi-webgpu: supported-features.has not implemented in matmul subset")
    }

    async fn drop(&mut self, rep: Resource<GpuSupportedFeatures>) -> wasmtime::Result<()> {
        self.resource_table.delete(rep)?;
        Ok(())
    }
}

impl HostGpuSupportedLimits for HostState {
    async fn drop(&mut self, rep: Resource<GpuSupportedLimits>) -> wasmtime::Result<()> {
        self.resource_table.delete(rep)?;
        Ok(())
    }

    async fn max_texture_dimension1_d(&mut self, _rep: Resource<GpuSupportedLimits>) -> u32 {
        unimplemented!("wasi-webgpu: limits accessor not implemented in matmul subset")
    }
    async fn max_texture_dimension2_d(&mut self, _rep: Resource<GpuSupportedLimits>) -> u32 {
        unimplemented!()
    }
    async fn max_texture_dimension3_d(&mut self, _rep: Resource<GpuSupportedLimits>) -> u32 {
        unimplemented!()
    }
    async fn max_texture_array_layers(&mut self, _rep: Resource<GpuSupportedLimits>) -> u32 {
        unimplemented!()
    }
    async fn max_bind_groups(&mut self, _rep: Resource<GpuSupportedLimits>) -> u32 {
        unimplemented!()
    }
    async fn max_bind_groups_plus_vertex_buffers(&mut self, _rep: Resource<GpuSupportedLimits>) -> u32 {
        unimplemented!()
    }
    async fn max_bindings_per_bind_group(&mut self, _rep: Resource<GpuSupportedLimits>) -> u32 {
        unimplemented!()
    }
    async fn max_dynamic_uniform_buffers_per_pipeline_layout(&mut self, _rep: Resource<GpuSupportedLimits>) -> u32 {
        unimplemented!()
    }
    async fn max_dynamic_storage_buffers_per_pipeline_layout(&mut self, _rep: Resource<GpuSupportedLimits>) -> u32 {
        unimplemented!()
    }
    async fn max_sampled_textures_per_shader_stage(&mut self, _rep: Resource<GpuSupportedLimits>) -> u32 {
        unimplemented!()
    }
    async fn max_samplers_per_shader_stage(&mut self, _rep: Resource<GpuSupportedLimits>) -> u32 {
        unimplemented!()
    }
    async fn max_storage_buffers_per_shader_stage(&mut self, _rep: Resource<GpuSupportedLimits>) -> u32 {
        unimplemented!()
    }
    async fn max_storage_textures_per_shader_stage(&mut self, _rep: Resource<GpuSupportedLimits>) -> u32 {
        unimplemented!()
    }
    async fn max_uniform_buffers_per_shader_stage(&mut self, _rep: Resource<GpuSupportedLimits>) -> u32 {
        unimplemented!()
    }
    async fn max_uniform_buffer_binding_size(&mut self, _rep: Resource<GpuSupportedLimits>) -> u64 {
        unimplemented!()
    }
    async fn max_storage_buffer_binding_size(&mut self, _rep: Resource<GpuSupportedLimits>) -> u64 {
        unimplemented!()
    }
    async fn min_uniform_buffer_offset_alignment(&mut self, _rep: Resource<GpuSupportedLimits>) -> u32 {
        unimplemented!()
    }
    async fn min_storage_buffer_offset_alignment(&mut self, _rep: Resource<GpuSupportedLimits>) -> u32 {
        unimplemented!()
    }
    async fn max_vertex_buffers(&mut self, _rep: Resource<GpuSupportedLimits>) -> u32 {
        unimplemented!()
    }
    async fn max_buffer_size(&mut self, _rep: Resource<GpuSupportedLimits>) -> u64 {
        unimplemented!()
    }
    async fn max_vertex_attributes(&mut self, _rep: Resource<GpuSupportedLimits>) -> u32 {
        unimplemented!()
    }
    async fn max_vertex_buffer_array_stride(&mut self, _rep: Resource<GpuSupportedLimits>) -> u32 {
        unimplemented!()
    }
    async fn max_inter_stage_shader_variables(&mut self, _rep: Resource<GpuSupportedLimits>) -> u32 {
        unimplemented!()
    }
    async fn max_color_attachments(&mut self, _rep: Resource<GpuSupportedLimits>) -> u32 {
        unimplemented!()
    }
    async fn max_color_attachment_bytes_per_sample(&mut self, _rep: Resource<GpuSupportedLimits>) -> u32 {
        unimplemented!()
    }
    async fn max_compute_workgroup_storage_size(&mut self, _rep: Resource<GpuSupportedLimits>) -> u32 {
        unimplemented!()
    }
    async fn max_compute_invocations_per_workgroup(&mut self, _rep: Resource<GpuSupportedLimits>) -> u32 {
        unimplemented!()
    }
    async fn max_compute_workgroup_size_x(&mut self, _rep: Resource<GpuSupportedLimits>) -> u32 {
        unimplemented!()
    }
    async fn max_compute_workgroup_size_y(&mut self, _rep: Resource<GpuSupportedLimits>) -> u32 {
        unimplemented!()
    }
    async fn max_compute_workgroup_size_z(&mut self, _rep: Resource<GpuSupportedLimits>) -> u32 {
        unimplemented!()
    }
    async fn max_compute_workgroups_per_dimension(&mut self, _rep: Resource<GpuSupportedLimits>) -> u32 {
        unimplemented!()
    }
}

impl HostGpuDevice for HostState {
    async fn queue(&mut self, rep: Resource<GpuDevice>) -> Resource<GpuQueue> {
        let dev = self.resource_table.get(&rep).expect("device handle");
        let (device, queue) = (dev.device.clone(), dev.queue.clone());
        self.resource_table
            .push(GpuQueue { device, queue })
            .expect("resource table push")
    }

    async fn create_buffer(
        &mut self,
        rep: Resource<GpuDevice>,
        descriptor: GpuBufferDescriptor,
    ) -> Resource<GpuBuffer> {
        let dev = self.resource_table.get(&rep).expect("device handle");
        let device = dev.device.clone();
        let usage_flags = descriptor.usage;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: descriptor.label.as_deref(),
            size: descriptor.size,
            usage: buffer_usage_from_flags(usage_flags),
            mapped_at_creation: descriptor.mapped_at_creation.unwrap_or(false),
        });
        let map_state = if descriptor.mapped_at_creation.unwrap_or(false) {
            GpuBufferMapState::Mapped
        } else {
            GpuBufferMapState::Unmapped
        };
        self.resource_table
            .push(GpuBuffer {
                buffer: Arc::new(buffer),
                device,
                size: descriptor.size,
                usage: usage_flags,
                map_state,
            })
            .expect("resource table push")
    }

    async fn create_bind_group_layout(
        &mut self,
        rep: Resource<GpuDevice>,
        descriptor: GpuBindGroupLayoutDescriptor,
    ) -> Resource<GpuBindGroupLayout> {
        let device = self.resource_table.get(&rep).expect("device handle").device.clone();
        let entries: Vec<wgpu::BindGroupLayoutEntry> = descriptor
            .entries
            .into_iter()
            .map(|e| {
                let buffer = e
                    .buffer
                    .map(|b| wgpu::BindingType::Buffer {
                        ty: buffer_binding_type(b.type_),
                        has_dynamic_offset: b.has_dynamic_offset.unwrap_or(false),
                        min_binding_size: b.min_binding_size.and_then(std::num::NonZeroU64::new),
                    })
                    .expect("matmul subset only uses buffer bindings");
                wgpu::BindGroupLayoutEntry {
                    binding: e.binding,
                    visibility: shader_stages_from_flags(e.visibility),
                    ty: buffer,
                    count: None,
                }
            })
            .collect();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: descriptor.label.as_deref(),
            entries: &entries,
        });
        self.resource_table
            .push(GpuBindGroupLayout {
                layout: Arc::new(layout),
            })
            .expect("resource table push")
    }

    async fn create_pipeline_layout(
        &mut self,
        rep: Resource<GpuDevice>,
        descriptor: GpuPipelineLayoutDescriptor,
    ) -> Resource<GpuPipelineLayout> {
        let device = self.resource_table.get(&rep).expect("device handle").device.clone();
        let layouts_owned: Vec<Arc<wgpu::BindGroupLayout>> = descriptor
            .bind_group_layouts
            .iter()
            .map(|b| {
                let b = b.as_ref().expect("matmul: pipeline layout entries must be Some");
                self.resource_table
                    .get(b)
                    .expect("bind-group-layout handle")
                    .layout
                    .clone()
            })
            .collect();
        let layout_refs: Vec<Option<&wgpu::BindGroupLayout>> = layouts_owned.iter().map(|a| Some(a.as_ref())).collect();
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: descriptor.label.as_deref(),
            bind_group_layouts: &layout_refs,
            immediate_size: 0,
        });
        self.resource_table
            .push(GpuPipelineLayout {
                layout: Arc::new(layout),
            })
            .expect("resource table push")
    }

    async fn create_bind_group(
        &mut self,
        rep: Resource<GpuDevice>,
        descriptor: GpuBindGroupDescriptor,
    ) -> Resource<GpuBindGroup> {
        let device = self.resource_table.get(&rep).expect("device handle").device.clone();
        let layout = self
            .resource_table
            .get(&descriptor.layout)
            .expect("bind-group-layout handle")
            .layout
            .clone();
        // Two passes: first materialize the (buffer Arc, offset, size) tuples,
        // then borrow the buffers into BindingResource::Buffer with lifetimes
        // that outlive the create_bind_group call.
        let mut buffer_keep: Vec<(Arc<wgpu::Buffer>, u64, Option<std::num::NonZeroU64>)> =
            Vec::with_capacity(descriptor.entries.len());
        let mut bindings: Vec<(u32, usize)> = Vec::with_capacity(descriptor.entries.len());
        for e in &descriptor.entries {
            match &e.resource {
                wg::GpuBindingResource::GpuBufferBinding(b) => {
                    let buf = self
                        .resource_table
                        .get(&b.buffer)
                        .expect("buffer handle")
                        .buffer
                        .clone();
                    let offset = b.offset.unwrap_or(0);
                    let size = b.size.and_then(std::num::NonZeroU64::new);
                    buffer_keep.push((buf, offset, size));
                    bindings.push((e.binding, buffer_keep.len() - 1));
                }
            }
        }
        let entries: Vec<wgpu::BindGroupEntry> = bindings
            .iter()
            .map(|(binding, idx)| {
                let (buf, offset, size) = &buffer_keep[*idx];
                wgpu::BindGroupEntry {
                    binding: *binding,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: buf.as_ref(),
                        offset: *offset,
                        size: *size,
                    }),
                }
            })
            .collect();
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: descriptor.label.as_deref(),
            layout: layout.as_ref(),
            entries: &entries,
        });
        self.resource_table
            .push(GpuBindGroup { group: Arc::new(group) })
            .expect("resource table push")
    }

    async fn create_shader_module(
        &mut self,
        rep: Resource<GpuDevice>,
        descriptor: GpuShaderModuleDescriptor,
    ) -> Resource<GpuShaderModule> {
        let device = self.resource_table.get(&rep).expect("device handle").device.clone();
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: descriptor.label.as_deref(),
            source: wgpu::ShaderSource::Wgsl(descriptor.code.into()),
        });
        self.resource_table
            .push(GpuShaderModule {
                module: Arc::new(module),
            })
            .expect("resource table push")
    }

    async fn create_compute_pipeline(
        &mut self,
        rep: Resource<GpuDevice>,
        descriptor: GpuComputePipelineDescriptor,
    ) -> Resource<GpuComputePipeline> {
        let device = self.resource_table.get(&rep).expect("device handle").device.clone();
        let module = self
            .resource_table
            .get(&descriptor.compute.module)
            .expect("shader-module handle")
            .module
            .clone();
        let layout: Option<Arc<wgpu::PipelineLayout>> = match &descriptor.layout {
            GpuLayoutMode::Specific(l) => Some(
                self.resource_table
                    .get(l)
                    .expect("pipeline-layout handle")
                    .layout
                    .clone(),
            ),
            GpuLayoutMode::Auto => None,
        };
        let entry_point = descriptor.compute.entry_point.as_deref();
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: descriptor.label.as_deref(),
            layout: layout.as_deref().map(|a| a as &wgpu::PipelineLayout),
            module: module.as_ref(),
            entry_point,
            compilation_options: Default::default(),
            cache: None,
        });
        self.resource_table
            .push(GpuComputePipeline {
                pipeline: Arc::new(pipeline),
            })
            .expect("resource table push")
    }

    async fn create_command_encoder(
        &mut self,
        rep: Resource<GpuDevice>,
        descriptor: Option<wg::GpuCommandEncoderDescriptor>,
    ) -> Resource<GpuCommandEncoder> {
        let device = self.resource_table.get(&rep).expect("device handle").device.clone();
        let label = descriptor.as_ref().and_then(|d| d.label.as_deref());
        let encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label });
        self.resource_table
            .push(GpuCommandEncoder {
                device,
                encoder: Some(encoder),
                pending_pass: None,
            })
            .expect("resource table push")
    }

    async fn features(&mut self, _rep: Resource<GpuDevice>) -> Resource<GpuSupportedFeatures> {
        unimplemented!("wasi-webgpu: device.features not implemented in matmul subset")
    }
    async fn limits(&mut self, _rep: Resource<GpuDevice>) -> Resource<GpuSupportedLimits> {
        unimplemented!("wasi-webgpu: device.limits not implemented in matmul subset")
    }
    async fn adapter_info(&mut self, _rep: Resource<GpuDevice>) -> Resource<GpuAdapterInfo> {
        unimplemented!("wasi-webgpu: device.adapter-info not implemented in matmul subset")
    }
    async fn destroy(&mut self, _rep: Resource<GpuDevice>) {
        unimplemented!("wasi-webgpu: device.destroy not implemented (use resource drop instead)")
    }
    async fn label(&mut self, _rep: Resource<GpuDevice>) -> String {
        unimplemented!("wasi-webgpu: device.label not implemented in matmul subset")
    }
    async fn set_label(&mut self, _rep: Resource<GpuDevice>, _label: String) {
        unimplemented!("wasi-webgpu: device.set-label not implemented in matmul subset")
    }

    async fn drop(&mut self, rep: Resource<GpuDevice>) -> wasmtime::Result<()> {
        self.resource_table.delete(rep)?;
        Ok(())
    }
}

impl HostGpuBuffer for HostState {
    async fn map_async(
        &mut self,
        rep: Resource<GpuBuffer>,
        _mode: u32,
        offset: Option<u64>,
        size: Option<u64>,
    ) -> Result<(), MapAsyncError> {
        let buf = self.resource_table.get(&rep).expect("buffer handle");
        let buffer = buf.buffer.clone();
        let device = buf.device.clone();
        let total_size = buf.size;
        let offset = offset.unwrap_or(0);
        let size = size.unwrap_or(total_size - offset);
        let result = tokio::task::spawn_blocking(move || {
            let (tx, rx) = std::sync::mpsc::channel();
            buffer
                .slice(offset..offset + size)
                .map_async(wgpu::MapMode::Read, move |r| {
                    let _ = tx.send(r);
                });
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .wit_context("device poll")?;
            rx.recv().wit_context("map_async channel")?.wit_context("map_async")
        })
        .await;
        match result {
            Ok(Ok(())) => {
                let buf = self.resource_table.get_mut(&rep).expect("buffer handle");
                buf.map_state = GpuBufferMapState::Mapped;
                Ok(())
            }
            Ok(Err(msg)) => Err(MapAsyncError {
                kind: MapAsyncErrorKind::OperationError,
                message: msg,
            }),
            Err(join_err) => Err(MapAsyncError {
                kind: MapAsyncErrorKind::OperationError,
                message: format!("map_async task join: {join_err}"),
            }),
        }
    }

    async fn get_mapped_range_get_with_copy(
        &mut self,
        rep: Resource<GpuBuffer>,
        offset: Option<u64>,
        size: Option<u64>,
    ) -> Result<Vec<u8>, GetMappedRangeError> {
        let buf = self.resource_table.get(&rep).expect("buffer handle");
        let offset = offset.unwrap_or(0);
        let size = size.unwrap_or(buf.size - offset);
        let slice = buf.buffer.slice(offset..offset + size);
        let data = slice.get_mapped_range();
        let bytes = data.to_vec();
        drop(data);
        Ok(bytes)
    }

    async fn unmap(&mut self, rep: Resource<GpuBuffer>) -> Result<(), UnmapError> {
        let buf = self.resource_table.get_mut(&rep).expect("buffer handle");
        buf.buffer.unmap();
        buf.map_state = GpuBufferMapState::Unmapped;
        Ok(())
    }

    async fn destroy(&mut self, rep: Resource<GpuBuffer>) {
        let buf = self.resource_table.get(&rep).expect("buffer handle");
        buf.buffer.destroy();
    }

    async fn size(&mut self, _rep: Resource<GpuBuffer>) -> u64 {
        unimplemented!("wasi-webgpu: buffer.size not implemented in matmul subset")
    }
    async fn usage(&mut self, _rep: Resource<GpuBuffer>) -> u32 {
        unimplemented!("wasi-webgpu: buffer.usage not implemented in matmul subset")
    }
    async fn map_state(&mut self, _rep: Resource<GpuBuffer>) -> GpuBufferMapState {
        unimplemented!("wasi-webgpu: buffer.map-state not implemented in matmul subset")
    }
    async fn label(&mut self, _rep: Resource<GpuBuffer>) -> String {
        unimplemented!("wasi-webgpu: buffer.label not implemented in matmul subset")
    }
    async fn set_label(&mut self, _rep: Resource<GpuBuffer>, _label: String) {
        unimplemented!("wasi-webgpu: buffer.set-label not implemented in matmul subset")
    }
    async fn get_mapped_range_set_with_copy(
        &mut self,
        _rep: Resource<GpuBuffer>,
        _data: Vec<u8>,
        _offset: Option<u64>,
        _size: Option<u64>,
    ) -> Result<(), GetMappedRangeError> {
        unimplemented!("wasi-webgpu: buffer.get-mapped-range-set-with-copy not implemented in matmul subset")
    }

    async fn drop(&mut self, rep: Resource<GpuBuffer>) -> wasmtime::Result<()> {
        self.resource_table.delete(rep)?;
        Ok(())
    }
}

impl HostGpuBufferUsage for HostState {
    async fn map_read(&mut self) -> u32 {
        Usage::MAP_READ
    }
    async fn map_write(&mut self) -> u32 {
        Usage::MAP_WRITE
    }
    async fn copy_src(&mut self) -> u32 {
        Usage::COPY_SRC
    }
    async fn copy_dst(&mut self) -> u32 {
        Usage::COPY_DST
    }
    async fn index(&mut self) -> u32 {
        Usage::INDEX
    }
    async fn vertex(&mut self) -> u32 {
        Usage::VERTEX
    }
    async fn uniform(&mut self) -> u32 {
        Usage::UNIFORM
    }
    async fn storage(&mut self) -> u32 {
        Usage::STORAGE
    }
    async fn indirect(&mut self) -> u32 {
        Usage::INDIRECT
    }
    async fn query_resolve(&mut self) -> u32 {
        Usage::QUERY_RESOLVE
    }

    async fn drop(&mut self, rep: Resource<GpuBufferUsage>) -> wasmtime::Result<()> {
        self.resource_table.delete(rep)?;
        Ok(())
    }
}

impl HostGpuMapMode for HostState {
    async fn read(&mut self) -> u32 {
        MapMode::READ
    }
    async fn write(&mut self) -> u32 {
        MapMode::WRITE
    }

    async fn drop(&mut self, rep: Resource<GpuMapMode>) -> wasmtime::Result<()> {
        self.resource_table.delete(rep)?;
        Ok(())
    }
}

impl HostGpuShaderStage for HostState {
    async fn vertex(&mut self) -> u32 {
        ShaderStage::VERTEX
    }
    async fn fragment(&mut self) -> u32 {
        ShaderStage::FRAGMENT
    }
    async fn compute(&mut self) -> u32 {
        ShaderStage::COMPUTE
    }

    async fn drop(&mut self, rep: Resource<GpuShaderStage>) -> wasmtime::Result<()> {
        self.resource_table.delete(rep)?;
        Ok(())
    }
}

impl HostGpuBindGroupLayout for HostState {
    async fn label(&mut self, _rep: Resource<GpuBindGroupLayout>) -> String {
        unimplemented!("wasi-webgpu: bind-group-layout.label not implemented")
    }
    async fn set_label(&mut self, _rep: Resource<GpuBindGroupLayout>, _label: String) {
        unimplemented!("wasi-webgpu: bind-group-layout.set-label not implemented")
    }
    async fn drop(&mut self, rep: Resource<GpuBindGroupLayout>) -> wasmtime::Result<()> {
        self.resource_table.delete(rep)?;
        Ok(())
    }
}

impl HostGpuBindGroup for HostState {
    async fn label(&mut self, _rep: Resource<GpuBindGroup>) -> String {
        unimplemented!("wasi-webgpu: bind-group.label not implemented")
    }
    async fn set_label(&mut self, _rep: Resource<GpuBindGroup>, _label: String) {
        unimplemented!("wasi-webgpu: bind-group.set-label not implemented")
    }
    async fn drop(&mut self, rep: Resource<GpuBindGroup>) -> wasmtime::Result<()> {
        self.resource_table.delete(rep)?;
        Ok(())
    }
}

impl HostGpuPipelineLayout for HostState {
    async fn label(&mut self, _rep: Resource<GpuPipelineLayout>) -> String {
        unimplemented!("wasi-webgpu: pipeline-layout.label not implemented")
    }
    async fn set_label(&mut self, _rep: Resource<GpuPipelineLayout>, _label: String) {
        unimplemented!("wasi-webgpu: pipeline-layout.set-label not implemented")
    }
    async fn drop(&mut self, rep: Resource<GpuPipelineLayout>) -> wasmtime::Result<()> {
        self.resource_table.delete(rep)?;
        Ok(())
    }
}

impl HostGpuShaderModule for HostState {
    async fn label(&mut self, _rep: Resource<GpuShaderModule>) -> String {
        unimplemented!("wasi-webgpu: shader-module.label not implemented")
    }
    async fn set_label(&mut self, _rep: Resource<GpuShaderModule>, _label: String) {
        unimplemented!("wasi-webgpu: shader-module.set-label not implemented")
    }
    async fn drop(&mut self, rep: Resource<GpuShaderModule>) -> wasmtime::Result<()> {
        self.resource_table.delete(rep)?;
        Ok(())
    }
}

impl HostGpuComputePipeline for HostState {
    async fn label(&mut self, _rep: Resource<GpuComputePipeline>) -> String {
        unimplemented!("wasi-webgpu: compute-pipeline.label not implemented")
    }
    async fn set_label(&mut self, _rep: Resource<GpuComputePipeline>, _label: String) {
        unimplemented!("wasi-webgpu: compute-pipeline.set-label not implemented")
    }
    async fn get_bind_group_layout(
        &mut self,
        _rep: Resource<GpuComputePipeline>,
        _index: u32,
    ) -> Resource<GpuBindGroupLayout> {
        unimplemented!("wasi-webgpu: compute-pipeline.get-bind-group-layout not implemented in matmul subset")
    }
    async fn drop(&mut self, rep: Resource<GpuComputePipeline>) -> wasmtime::Result<()> {
        self.resource_table.delete(rep)?;
        Ok(())
    }
}

impl HostGpuCommandBuffer for HostState {
    async fn label(&mut self, _rep: Resource<GpuCommandBuffer>) -> String {
        unimplemented!("wasi-webgpu: command-buffer.label not implemented")
    }
    async fn set_label(&mut self, _rep: Resource<GpuCommandBuffer>, _label: String) {
        unimplemented!("wasi-webgpu: command-buffer.set-label not implemented")
    }
    async fn drop(&mut self, rep: Resource<GpuCommandBuffer>) -> wasmtime::Result<()> {
        self.resource_table.delete(rep)?;
        Ok(())
    }
}

impl HostGpuCommandEncoder for HostState {
    async fn begin_compute_pass(
        &mut self,
        rep: Resource<GpuCommandEncoder>,
        _descriptor: Option<GpuComputePassDescriptor>,
    ) -> Resource<GpuComputePassEncoder> {
        let encoder_rep = rep.rep();
        let enc = self.resource_table.get_mut(&rep).expect("encoder handle");
        assert!(
            enc.pending_pass.is_none(),
            "wasi-webgpu: nested compute passes not supported"
        );
        enc.pending_pass = Some(Vec::new());
        self.resource_table
            .push(GpuComputePassEncoder {
                encoder_rep,
                ended: false,
            })
            .expect("resource table push")
    }

    async fn copy_buffer_to_buffer(
        &mut self,
        rep: Resource<GpuCommandEncoder>,
        source: Resource<GpuBuffer>,
        source_offset: u64,
        destination: Resource<GpuBuffer>,
        destination_offset: u64,
        size: u64,
    ) {
        let src = self
            .resource_table
            .get(&source)
            .expect("src buffer handle")
            .buffer
            .clone();
        let dst = self
            .resource_table
            .get(&destination)
            .expect("dst buffer handle")
            .buffer
            .clone();
        let enc = self.resource_table.get_mut(&rep).expect("encoder handle");
        let encoder = enc.encoder.as_mut().expect("encoder already finished");
        encoder.copy_buffer_to_buffer(src.as_ref(), source_offset, dst.as_ref(), destination_offset, size);
    }

    async fn finish(
        &mut self,
        rep: Resource<GpuCommandEncoder>,
        _descriptor: Option<wg::GpuCommandBufferDescriptor>,
    ) -> Resource<GpuCommandBuffer> {
        let enc = self.resource_table.get_mut(&rep).expect("encoder handle");
        let encoder = enc.encoder.take().expect("encoder already finished");
        let buffer = encoder.finish();
        self.resource_table
            .push(GpuCommandBuffer { buffer: Some(buffer) })
            .expect("resource table push")
    }

    async fn label(&mut self, _rep: Resource<GpuCommandEncoder>) -> String {
        unimplemented!("wasi-webgpu: command-encoder.label not implemented")
    }
    async fn set_label(&mut self, _rep: Resource<GpuCommandEncoder>, _label: String) {
        unimplemented!("wasi-webgpu: command-encoder.set-label not implemented")
    }
    async fn drop(&mut self, rep: Resource<GpuCommandEncoder>) -> wasmtime::Result<()> {
        self.resource_table.delete(rep)?;
        Ok(())
    }
}

fn encoder_from_pass_rep(state: &mut HostState, pass: &Resource<GpuComputePassEncoder>) -> u32 {
    state.resource_table.get(pass).expect("pass handle").encoder_rep
}

impl HostGpuComputePassEncoder for HostState {
    async fn set_pipeline(&mut self, rep: Resource<GpuComputePassEncoder>, pipeline: Resource<wg::GpuComputePipeline>) {
        let pipe = self
            .resource_table
            .get(&pipeline)
            .expect("pipeline handle")
            .pipeline
            .clone();
        let encoder_rep = encoder_from_pass_rep(self, &rep);
        let enc_handle: Resource<GpuCommandEncoder> = Resource::new_borrow(encoder_rep);
        let enc = self.resource_table.get_mut(&enc_handle).expect("encoder handle");
        enc.pending_pass
            .as_mut()
            .expect("compute pass not active")
            .push(PassCommand::SetPipeline(pipe));
    }

    async fn set_bind_group(
        &mut self,
        rep: Resource<GpuComputePassEncoder>,
        index: u32,
        bind_group: Option<Resource<GpuBindGroup>>,
        _dynamic_offsets_data: Option<Vec<u32>>,
        _dynamic_offsets_data_start: Option<u64>,
        _dynamic_offsets_data_length: Option<u32>,
    ) -> Result<(), SetBindGroupError> {
        let bg = bind_group.ok_or_else(|| SetBindGroupError {
            kind: SetBindGroupErrorKind::RangeError,
            message: "set-bind-group with None not supported in matmul subset".into(),
        })?;
        let group = self.resource_table.get(&bg).expect("bind-group handle").group.clone();
        let encoder_rep = encoder_from_pass_rep(self, &rep);
        let enc_handle: Resource<GpuCommandEncoder> = Resource::new_borrow(encoder_rep);
        let enc = self.resource_table.get_mut(&enc_handle).expect("encoder handle");
        enc.pending_pass
            .as_mut()
            .expect("compute pass not active")
            .push(PassCommand::SetBindGroup {
                index,
                group,
                offsets: Vec::new(),
            });
        Ok(())
    }

    async fn dispatch_workgroups(
        &mut self,
        rep: Resource<GpuComputePassEncoder>,
        x: u32,
        y: Option<u32>,
        z: Option<u32>,
    ) {
        let encoder_rep = encoder_from_pass_rep(self, &rep);
        let enc_handle: Resource<GpuCommandEncoder> = Resource::new_borrow(encoder_rep);
        let enc = self.resource_table.get_mut(&enc_handle).expect("encoder handle");
        enc.pending_pass
            .as_mut()
            .expect("compute pass not active")
            .push(PassCommand::DispatchWorkgroups(x, y.unwrap_or(1), z.unwrap_or(1)));
    }

    async fn end(&mut self, rep: Resource<GpuComputePassEncoder>) {
        let encoder_rep = {
            let pass = self.resource_table.get_mut(&rep).expect("pass handle");
            if pass.ended {
                return;
            }
            pass.ended = true;
            pass.encoder_rep
        };
        let enc_handle: Resource<GpuCommandEncoder> = Resource::new_borrow(encoder_rep);
        let commands = {
            let enc = self.resource_table.get_mut(&enc_handle).expect("encoder handle");
            enc.pending_pass.take().expect("compute pass not active at end()")
        };
        let enc = self.resource_table.get_mut(&enc_handle).expect("encoder handle");
        let encoder = enc.encoder.as_mut().expect("encoder already finished");
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("wasi-webgpu buffered pass"),
            timestamp_writes: None,
        });
        for cmd in commands {
            match cmd {
                PassCommand::SetPipeline(p) => pass.set_pipeline(p.as_ref()),
                PassCommand::SetBindGroup { index, group, offsets } => {
                    pass.set_bind_group(index, group.as_ref(), &offsets);
                }
                PassCommand::DispatchWorkgroups(x, y, z) => pass.dispatch_workgroups(x, y, z),
            }
        }
    }

    async fn label(&mut self, _rep: Resource<GpuComputePassEncoder>) -> String {
        unimplemented!("wasi-webgpu: compute-pass.label not implemented")
    }
    async fn set_label(&mut self, _rep: Resource<GpuComputePassEncoder>, _label: String) {
        unimplemented!("wasi-webgpu: compute-pass.set-label not implemented")
    }

    async fn drop(&mut self, rep: Resource<GpuComputePassEncoder>) -> wasmtime::Result<()> {
        self.resource_table.delete(rep)?;
        Ok(())
    }
}

impl HostGpuQueue for HostState {
    async fn submit(&mut self, rep: Resource<GpuQueue>, command_buffers: Vec<Resource<wg::GpuCommandBuffer>>) {
        let queue = self.resource_table.get(&rep).expect("queue handle").queue.clone();
        let mut buffers: Vec<wgpu::CommandBuffer> = Vec::with_capacity(command_buffers.len());
        for cb_res in command_buffers {
            let cb = self
                .resource_table
                .get_mut(&cb_res)
                .expect("command-buffer handle")
                .buffer
                .take()
                .expect("command buffer already submitted");
            buffers.push(cb);
        }
        queue.submit(buffers);
    }

    async fn write_buffer_with_copy(
        &mut self,
        rep: Resource<GpuQueue>,
        buffer: Resource<wg::GpuBuffer>,
        buffer_offset: u64,
        data: Vec<u8>,
        data_offset: Option<u64>,
        size: Option<u64>,
    ) -> Result<(), WriteBufferError> {
        let queue = self.resource_table.get(&rep).expect("queue handle").queue.clone();
        let buf = self.resource_table.get(&buffer).expect("buffer handle").buffer.clone();
        let data_offset = data_offset.unwrap_or(0) as usize;
        let end = size.map_or(data.len(), |s| data_offset + s as usize);
        if end > data.len() {
            return Err(WriteBufferError {
                kind: WriteBufferErrorKind::OperationError,
                message: format!("data slice [{data_offset}..{end}] exceeds source len {}", data.len()),
            });
        }
        queue.write_buffer(buf.as_ref(), buffer_offset, &data[data_offset..end]);
        Ok(())
    }

    async fn label(&mut self, _rep: Resource<GpuQueue>) -> String {
        unimplemented!("wasi-webgpu: queue.label not implemented")
    }
    async fn set_label(&mut self, _rep: Resource<GpuQueue>, _label: String) {
        unimplemented!("wasi-webgpu: queue.set-label not implemented")
    }

    async fn drop(&mut self, rep: Resource<GpuQueue>) -> wasmtime::Result<()> {
        self.resource_table.delete(rep)?;
        Ok(())
    }
}

impl HostRecordOptionGpuSize64 for HostState {
    async fn new(&mut self) -> Resource<RecordOptionGpuSize64> {
        self.resource_table
            .push(RecordOptionGpuSize64 { map: BTreeMap::new() })
            .expect("resource table push")
    }
    async fn add(&mut self, rep: Resource<RecordOptionGpuSize64>, key: String, value: Option<u64>) {
        self.resource_table
            .get_mut(&rep)
            .expect("record handle")
            .map
            .insert(key, value);
    }
    async fn get(&mut self, rep: Resource<RecordOptionGpuSize64>, key: String) -> Option<Option<u64>> {
        self.resource_table
            .get(&rep)
            .expect("record handle")
            .map
            .get(&key)
            .copied()
    }
    async fn has(&mut self, rep: Resource<RecordOptionGpuSize64>, key: String) -> bool {
        self.resource_table
            .get(&rep)
            .expect("record handle")
            .map
            .contains_key(&key)
    }
    async fn remove(&mut self, rep: Resource<RecordOptionGpuSize64>, key: String) {
        self.resource_table
            .get_mut(&rep)
            .expect("record handle")
            .map
            .remove(&key);
    }
    async fn keys(&mut self, rep: Resource<RecordOptionGpuSize64>) -> Vec<String> {
        self.resource_table
            .get(&rep)
            .expect("record handle")
            .map
            .keys()
            .cloned()
            .collect()
    }
    async fn values(&mut self, rep: Resource<RecordOptionGpuSize64>) -> Vec<Option<u64>> {
        self.resource_table
            .get(&rep)
            .expect("record handle")
            .map
            .values()
            .copied()
            .collect()
    }
    async fn entries(&mut self, rep: Resource<RecordOptionGpuSize64>) -> Vec<(String, Option<u64>)> {
        self.resource_table
            .get(&rep)
            .expect("record handle")
            .map
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect()
    }
    async fn drop(&mut self, rep: Resource<RecordOptionGpuSize64>) -> wasmtime::Result<()> {
        self.resource_table.delete(rep)?;
        Ok(())
    }
}

impl HostRecordGpuPipelineConstantValue for HostState {
    async fn new(&mut self) -> Resource<RecordGpuPipelineConstantValue> {
        self.resource_table
            .push(RecordGpuPipelineConstantValue { map: BTreeMap::new() })
            .expect("resource table push")
    }
    async fn add(&mut self, rep: Resource<RecordGpuPipelineConstantValue>, key: String, value: f64) {
        self.resource_table
            .get_mut(&rep)
            .expect("record handle")
            .map
            .insert(key, value);
    }
    async fn get(&mut self, rep: Resource<RecordGpuPipelineConstantValue>, key: String) -> Option<f64> {
        self.resource_table
            .get(&rep)
            .expect("record handle")
            .map
            .get(&key)
            .copied()
    }
    async fn has(&mut self, rep: Resource<RecordGpuPipelineConstantValue>, key: String) -> bool {
        self.resource_table
            .get(&rep)
            .expect("record handle")
            .map
            .contains_key(&key)
    }
    async fn remove(&mut self, rep: Resource<RecordGpuPipelineConstantValue>, key: String) {
        self.resource_table
            .get_mut(&rep)
            .expect("record handle")
            .map
            .remove(&key);
    }
    async fn keys(&mut self, rep: Resource<RecordGpuPipelineConstantValue>) -> Vec<String> {
        self.resource_table
            .get(&rep)
            .expect("record handle")
            .map
            .keys()
            .cloned()
            .collect()
    }
    async fn values(&mut self, rep: Resource<RecordGpuPipelineConstantValue>) -> Vec<f64> {
        self.resource_table
            .get(&rep)
            .expect("record handle")
            .map
            .values()
            .copied()
            .collect()
    }
    async fn entries(&mut self, rep: Resource<RecordGpuPipelineConstantValue>) -> Vec<(String, f64)> {
        self.resource_table
            .get(&rep)
            .expect("record handle")
            .map
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect()
    }
    async fn drop(&mut self, rep: Resource<RecordGpuPipelineConstantValue>) -> wasmtime::Result<()> {
        self.resource_table.delete(rep)?;
        Ok(())
    }
}

// Suppress unused-import warnings on error variants we only mention via the
// kept-but-unimplemented stubs above. Keeping the import list complete makes
// the file's WIT-mapped surface obvious at a glance. Static-allocated values
// avoid the const-drop restriction that bites composite types like
// `CreatePipelineError`.
const _: fn() = || {
    let _ = CreatePipelineError {
        kind: CreatePipelineErrorKind::GpuPipelineError(GpuPipelineErrorReason::Validation),
        message: String::new(),
    };
    let _ = GetMappedRangeErrorKind::OperationError;
    let _ = UnmapErrorKind::AbortError;
};
