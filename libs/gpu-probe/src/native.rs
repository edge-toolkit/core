//! Puts the probe sets to a GPU through wgpu, and reads what the device says it is.
//!
//! Built natively, wgpu opens the machine's GPUs itself and the device is described from the API it was opened through.
//! Built for `wasm32`, wgpu opens the browser's WebGPU, so the same probes run in a page. Every call that waits on the
//! GPU is async, since a page cannot block for it; a native caller blocks on each with an executor.
//!
//! The device is asked for the optional features the probes need when the adapter has them: rendering to
//! `rg11b10ufloat` for a clear, and `shader-f16` for the `f16` probe set. Without one, what needs it is refused, which
//! its signature records like any other refusal.
#![expect(
    clippy::single_call_fn,
    reason = "each step of opening the device, running a probe set and reading the device is a named function"
)]
#![cfg_attr(
    target_arch = "wasm32",
    expect(
        clippy::future_not_send,
        reason = "a page's WebGPU objects are JavaScript's, which stay on the one thread a page has"
    )
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[cfg(target_vendor = "apple")]
use objc2_metal::MTLDevice as _;

pub use crate::memory::{ApiMemory, NoApiMemory, VulkanHeap, WALK_RESOLUTION, Walk, vulkan_memory, walk_with};
use crate::r#unsafe;
use crate::{
    Accumulator, Answer, BLENDS, Blend, CLEARS, Clear, EVERY_SET, F16_INPUTS, FILTERS, Filter, Format, INPUTS,
    MATRIX_PROBES, MatrixProbe, Operation, PROBE_COUNT, RASTER_FORMAT, RASTER_WIDTH, RASTERS, Raster, Shape, Signature,
    TRIG_INPUTS, WEBGPU, WEBGPU_CLEAR, WEBGPU_COMBINED, WEBGPU_F16, WEBGPU_F16_COMBINED, WEBGPU_FIXED_FUNCTION,
    WEBGPU_TRIG, WGPU_COOPERATIVE_MATRIX, combined_shader, coverage, f16_combined_shader, f16_shaders, matrix_shaders,
    shaders, trig_shaders,
};

/// The optional features the device is asked for whenever the adapter has them.
///
/// Rendering to `rg11b10ufloat` is one clear's target, `shader-f16` is what the `f16` probe set compiles with, and
/// cooperative matrices are what the matrix probes run on.
const OPTIONAL_FEATURES: wgpu::Features = wgpu::Features::RG11B10UFLOAT_RENDERABLE
    .union(wgpu::Features::SHADER_F16)
    .union(wgpu::Features::EXPERIMENTAL_COOPERATIVE_MATRIX);

/// How long the probe waits for the GPU to finish a submission before reporting it unfinished.
///
/// Every probe set the bench runners record is answered within tens of milliseconds, so a device still busy after this
/// long has stopped answering, and [`NativeError::Poll`] says so where an unbounded wait hangs the caller for good.
pub(crate) const SUBMISSION_TIMEOUT: Duration = Duration::from_secs(10);

/// How many bytes the storage buffer every probe shader reads [`INPUTS`] from holds.
#[expect(
    clippy::as_conversions,
    reason = "a few dozen bytes fit any u64, and a const has no fallible conversion from usize"
)]
const INPUT_BYTES: u64 = size_of_val(&INPUTS) as u64;

/// How many bytes the one `u32` every probe shader writes takes.
const OUTPUT_BYTES: u64 = 4;

/// How many bytes the readback of a signature's answers holds: one word a probe.
const SIGNATURE_BYTES: u64 = SET_WORDS * OUTPUT_BYTES;

/// How many words a combined shader writes: one a probe.
#[expect(
    clippy::as_conversions,
    reason = "a few dozen probes fit any u64, and a const has no fallible conversion from usize"
)]
const SET_WORDS: u64 = PROBE_COUNT as u64;

/// How many bytes of the readback buffer each probe drawn into a target has.
///
/// A texture copy's offset into the buffer is aligned to this.
const SLOT_BYTES: u64 = 256;

/// The format a filter probe draws its sample into, read back as the f32 it is.
const FILTER_TARGET: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;

/// Each graphics API wgpu opens a device through, beside the name the registry gives it.
const BACKENDS: [(wgpu::Backend, &str); 4] = [
    (wgpu::Backend::Vulkan, "vulkan"),
    (wgpu::Backend::Metal, "metal"),
    (wgpu::Backend::Dx12, "d3d12"),
    (wgpu::Backend::Gl, "opengl"),
];

/// Every Metal GPU family, beside the name Apple gives it.
#[cfg(target_vendor = "apple")]
const METAL_FAMILIES: [(objc2_metal::MTLGPUFamily, &str); 16] = [
    (objc2_metal::MTLGPUFamily::Apple1, "apple1"),
    (objc2_metal::MTLGPUFamily::Apple2, "apple2"),
    (objc2_metal::MTLGPUFamily::Apple3, "apple3"),
    (objc2_metal::MTLGPUFamily::Apple4, "apple4"),
    (objc2_metal::MTLGPUFamily::Apple5, "apple5"),
    (objc2_metal::MTLGPUFamily::Apple6, "apple6"),
    (objc2_metal::MTLGPUFamily::Apple7, "apple7"),
    (objc2_metal::MTLGPUFamily::Apple8, "apple8"),
    (objc2_metal::MTLGPUFamily::Apple9, "apple9"),
    (objc2_metal::MTLGPUFamily::Apple10, "apple10"),
    (objc2_metal::MTLGPUFamily::Mac2, "mac2"),
    (objc2_metal::MTLGPUFamily::Common1, "common1"),
    (objc2_metal::MTLGPUFamily::Common2, "common2"),
    (objc2_metal::MTLGPUFamily::Common3, "common3"),
    (objc2_metal::MTLGPUFamily::Metal3, "metal3"),
    (objc2_metal::MTLGPUFamily::Metal4, "metal4"),
];

/// What stopped the probes being put to the GPU.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum NativeError {
    /// wgpu found no adapter to open.
    #[error("no GPU adapter could be opened: {0}")]
    Adapter(#[from] wgpu::RequestAdapterError),
    /// The adapter would not open a device.
    #[error("the GPU adapter would not open a device: {0}")]
    Device(#[from] wgpu::RequestDeviceError),
    /// Waiting for the device to finish the probes failed, or ran out of time.
    #[error("the GPU did not finish the probes: {0}")]
    Poll(#[from] wgpu::PollError),
    /// The buffer the answers were copied to could not be read.
    #[error("the GPU's answers could not be read back: {0}")]
    Readback(#[from] wgpu::BufferAsyncError),
    /// wgpu dropped the buffer's mapping callback without ever calling it.
    #[error("the GPU never said whether its answers could be read back: {0}")]
    Unmapped(#[from] futures_channel::oneshot::Canceled),
    /// The device was lost before it finished its work.
    ///
    /// wgpu says so natively by dropping the callback for that work uncalled, and in a page only through the device's
    /// lost callback.
    #[error("the GPU was lost before it finished its work")]
    Lost,
    /// The device runs no compute shaders, which a shader probe set is, for the reason given.
    #[error("the device runs no compute shaders: {0}")]
    Compute(#[from] NoCompute),
}

/// Why a device runs no compute shaders.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum NoCompute {
    /// The API gave a context whose shading language has no compute shaders, as [`runs_compute`] judges it.
    #[error("{backend} gives {version}")]
    Context {
        /// The backend's registry name, such as `opengl`.
        backend: &'static str,
        /// The context's version string.
        version: String,
    },
    /// The device was opened without them, by [`Gpu::open_without_compute`].
    #[error("{adapter} was opened without them")]
    NotAsked {
        /// wgpu's name for the adapter.
        adapter: String,
    },
}

/// A probe as a device holds it: the pipeline that evaluates it, or the device's reason for refusing to build one.
type Probe = Result<wgpu::ComputePipeline, wgpu::Error>;

/// What a device says it is, read from wgpu and from the API beneath it.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct Report {
    /// Every value, one `name: value` line each.
    pub lines: Vec<String>,
    /// The values a registry row has a field for, as far as the API gives them.
    ///
    /// Those are `driver`, `driver_version` and `api_version`.
    pub versions: Vec<(&'static str, String)>,
}

/// This machine's GPU, opened through wgpu, with the probe set compiled for it.
pub struct Gpu {
    /// The adapter the device was opened on.
    adapter: wgpu::Adapter,
    /// The shader probe sets compiled for the device, or why the device runs none.
    compute: Result<Compute, NoCompute>,
    /// The device the probes run on.
    device: wgpu::Device,
    /// Where the device's work is submitted.
    queue: wgpu::Queue,
}

/// The shader probe sets, each a compute pipeline, as a device compiled them.
struct Compute {
    /// The whole probe set as one shader.
    combined: Probe,
    /// The whole `f16` probe set as one shader.
    f16_combined: Probe,
    /// The `f16` probe set, in output order.
    f16_probes: Vec<Probe>,
    /// The matrix probes, in output order.
    matrix_probes: Vec<Probe>,
    /// The probe set, in output order.
    probes: Vec<Probe>,
    /// The `sin` and `cos` probes, in output order.
    trig_probes: Vec<Probe>,
}

/// How a probe drawn into a target reads its answer from the bytes of the target it copied back.
#[derive(Clone, Copy)]
enum Read {
    /// The red channel of the first texel, in the format given.
    Red(Format),
    /// The first four bytes, as the f32 an `r32float` texel holds.
    Word,
    /// The mask of the texels a raster probe's triangle covered.
    Coverage,
}

/// A probe drawn into a target: the target, how its pass starts, what the pass draws, and how its answer is read.
struct Pass {
    /// What the pass draws: a pipeline, and the bind group it reads, if it reads one. A clear draws nothing.
    draw: Option<(wgpu::RenderPipeline, Option<wgpu::BindGroup>)>,
    /// How the pass starts: clearing the target to a value, or keeping what was uploaded to it.
    load: wgpu::LoadOp<wgpu::Color>,
    /// How the answer is read from the target's bytes.
    read: Read,
    /// The target, one texel high.
    target: wgpu::Texture,
}

impl Gpu {
    /// Open the adapter wgpu picks by default, and compile the probe set for it.
    pub async fn open() -> Result<Self, NativeError> {
        Self::open_with(&wgpu::RequestAdapterOptions::default()).await
    }

    /// Open the adapter wgpu, or the browser behind it, chooses for `options`, with the probe set compiled for it.
    ///
    /// A browser offers its GPUs only this way, one for each power preference and a fallback adapter when asked for
    /// one, so asking with each is how a page reaches every GPU it can.
    pub async fn open_with(options: &wgpu::RequestAdapterOptions<'_, '_>) -> Result<Self, NativeError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance.request_adapter(options).await?;
        Self::open_adapter(adapter).await
    }

    /// Open every adapter wgpu finds, with the probe set compiled for each.
    ///
    /// wgpu finds them through Vulkan, Metal, Direct3D 12 and OpenGL. They come in the order wgpu lists them. A
    /// software renderer, such as llvmpipe or WARP, is opened beside the GPUs: it answers as the CPU's own arithmetic
    /// does through that renderer, which is a signature of its own. So is a GPU reached through two of those APIs,
    /// which is opened once through each: the driver beneath each API is its own, and can answer differently.
    ///
    /// Each adapter gives its own outcome, so one whose device will not open -- a broken driver, or a software renderer
    /// that rejects the default limits -- stops none of the others being measured.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn open_all() -> Vec<Result<Self, NativeError>> {
        let primary = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        // OpenGL is opened from an instance of its own, without wgpu-core's validation of indirect calls, which no
        // probe makes. That validation is a compute pipeline built as each device opens, and a context that runs no
        // compute shaders cannot build it, so the device would never open. NVIDIA's EGL gives wgpu 29.0.4 an OpenGL 3.3
        // context, whose GLSL 330 has no compute shaders; with NVIDIA's 580.178.04 driver on Linux, a Quadro M2000's
        // OpenGL adapter failed with indirect-validation error: ComputePipeline(Internal("The selected version doesn't
        // support Features(BUFFER_STORAGE | COMPUTE_SHADER | DYNAMIC_ARRAY_SIZE)")) and then `the GPU adapter would not
        // open a device: Parent device is lost`. The other APIs keep it: on Direct3D 12, turning it off also changes
        // what `vertex_index` reports.
        let mut without_indirect = wgpu::InstanceDescriptor::new_without_display_handle();
        without_indirect
            .flags
            .remove(wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL);
        let opengl = wgpu::Instance::new(without_indirect);
        let mut adapters = primary.enumerate_adapters(wgpu::Backends::PRIMARY).await;
        adapters.extend(opengl.enumerate_adapters(wgpu::Backends::GL).await);
        let mut gpus = Vec::new();
        for adapter in adapters {
            gpus.push(Self::open_adapter(adapter).await);
        }
        gpus
    }

    /// Open a device on `adapter`, and compile the probe set for it.
    ///
    /// For a caller that chose the adapter itself, from an instance of its own: one limited to a backend, say. A device
    /// that runs no compute shaders, as [`compute_shaders`] judges it, is opened as [`open_without_compute`] does.
    ///
    /// [`open_without_compute`]: Self::open_without_compute
    pub async fn open_adapter(adapter: wgpu::Adapter) -> Result<Self, NativeError> {
        let info = adapter.get_info();
        let flags = adapter.get_downlevel_capabilities().flags;
        let compute = compute_shaders(info.backend, flags, &info.driver_info);
        Self::open_device(adapter, compute).await
    }

    /// Open a device on `adapter` with no shader probe set compiled, for a context that runs no compute shaders.
    ///
    /// The clears and the fixed-function probes draw rather than compute, so they still answer; each shader probe set
    /// answers [`NativeError::Compute`] instead of the device failing as it compiles one.
    pub async fn open_without_compute(adapter: wgpu::Adapter) -> Result<Self, NativeError> {
        let reason = NoCompute::NotAsked {
            adapter: adapter.get_info().name,
        };
        Self::open_device(adapter, Err(reason)).await
    }

    /// Open a device on `adapter`, compiling the shader probe sets for it unless `compute` gives the reason not to.
    ///
    /// Cooperative matrices are asked for only where a matrix probe would run on them. Asked for anyway, a driver
    /// can offer them and then refuse the device: lavapipe from Mesa 26.0.8 on arm64 Ubuntu 26.04 did, and wgpu
    /// panicked with `wgpu-hal invariant was violated (usage error): Requested feature is not available on this
    /// device`, on <https://github.com/edge-toolkit/core/commit/a17610dd94f476d2b53b8660751d5ab058041e57> at
    /// <https://github.com/edge-toolkit/core/actions/runs/45729965729/job/139623710473>.
    async fn open_device(adapter: wgpu::Adapter, compute: Result<(), NoCompute>) -> Result<Self, NativeError> {
        let info = adapter.get_info();
        // Metal reports 4..=64 for every device whatever its simdgroups are. The only Metal GPUs offering a cooperative
        // matrix are Apple's own, whose simdgroups are 32 lanes, so there 32 is taken as what they are.
        let lanes = match info.backend {
            wgpu::Backend::Metal => [32, 32],
            wgpu::Backend::Noop
            | wgpu::Backend::Vulkan
            | wgpu::Backend::Dx12
            | wgpu::Backend::Gl
            | wgpu::Backend::BrowserWebGpu => [info.subgroup_min_size, info.subgroup_max_size],
        };
        let matrices = Matrices {
            lanes,
            offered: adapter.cooperative_matrix_properties(),
        };
        // Values rather than branches, as in `ensure_offered`, which says why.
        let runs_matrices = MATRIX_PROBES
            .iter()
            .any(|probe| ensure_offered(probe, &matrices).is_ok());
        let mut wanted = OPTIONAL_FEATURES;
        wanted.set(wgpu::Features::EXPERIMENTAL_COOPERATIVE_MATRIX, runs_matrices);
        let required_features = adapter.features() & wanted;
        let descriptor = wgpu::DeviceDescriptor {
            required_features,
            experimental_features: r#unsafe::experimental_features(),
            ..wgpu::DeviceDescriptor::default()
        };
        let (device, queue) = adapter.request_device(&descriptor).await?;
        let compute = match compute {
            Ok(()) => Ok(Compute::compile(&device, &matrices).await),
            Err(reason) => Err(reason),
        };
        Ok(Self {
            adapter,
            compute,
            device,
            queue,
        })
    }

    /// The shader probe sets compiled for the device, or the error saying why it has none.
    fn compute(&self) -> Result<&Compute, NativeError> {
        match &self.compute {
            Ok(compute) => Ok(compute),
            Err(reason) => Err(reason.clone().into()),
        }
    }

    /// What wgpu reports the adapter is.
    #[must_use]
    pub fn info(&self) -> wgpu::AdapterInfo {
        self.adapter.get_info()
    }

    /// Whether the device may render to `rg11b10ufloat`, without which that clear is refused.
    #[must_use]
    pub fn renders_ufloat11(&self) -> bool {
        self.device
            .features()
            .contains(wgpu::Features::RG11B10UFLOAT_RENDERABLE)
    }

    /// Whether the device has `shader-f16`, without which every probe of the `f16` set is refused.
    #[must_use]
    pub fn computes_f16(&self) -> bool {
        self.device.features().contains(wgpu::Features::SHADER_F16)
    }

    /// The name the registry gives the graphics API the device was opened through, or `unknown`.
    #[must_use]
    pub fn backend(&self) -> &'static str {
        let info = self.info();
        backend_name(info.backend, &info.driver_info)
    }

    /// What the probe set answers on this GPU, each probe in a shader of its own.
    pub async fn signature(&self) -> Result<Signature, NativeError> {
        let stages: Vec<(&Probe, u64)> = self.compute()?.probes.iter().map(|probe| (probe, 1)).collect();
        Ok(Signature::new(WEBGPU, self.answers(&stages, &INPUTS).await?))
    }

    /// What the probe set answers on this GPU, every probe in the one combined shader.
    pub async fn combined_signature(&self) -> Result<Signature, NativeError> {
        let answers = self.answers(&[(&self.compute()?.combined, SET_WORDS)], &INPUTS).await?;
        Ok(Signature::new(WEBGPU_COMBINED, answers))
    }

    /// What the `f16` probe set answers on this GPU, each probe in a shader of its own.
    pub async fn f16_signature(&self) -> Result<Signature, NativeError> {
        let stages: Vec<(&Probe, u64)> = self.compute()?.f16_probes.iter().map(|probe| (probe, 1)).collect();
        Ok(Signature::new(WEBGPU_F16, self.answers(&stages, &F16_INPUTS).await?))
    }

    /// What the `f16` probe set answers on this GPU, every probe in the one combined shader.
    pub async fn f16_combined_signature(&self) -> Result<Signature, NativeError> {
        let answers = self
            .answers(&[(&self.compute()?.f16_combined, SET_WORDS)], &F16_INPUTS)
            .await?;
        Ok(Signature::new(WEBGPU_F16_COMBINED, answers))
    }

    /// What the matrix probes answer on this GPU, each in a shader of its own.
    pub async fn matrix_signature(&self) -> Result<Signature, NativeError> {
        let stages: Vec<(&Probe, u64)> = self.compute()?.matrix_probes.iter().map(|probe| (probe, 1)).collect();
        Ok(Signature::new(
            WGPU_COOPERATIVE_MATRIX,
            self.answers(&stages, &INPUTS).await?,
        ))
    }

    /// What the `sin` and `cos` probes answer on this GPU, each in a shader of its own.
    pub async fn trig_signature(&self) -> Result<Signature, NativeError> {
        let stages: Vec<(&Probe, u64)> = self.compute()?.trig_probes.iter().map(|probe| (probe, 1)).collect();
        Ok(Signature::new(WEBGPU_TRIG, self.answers(&stages, &TRIG_INPUTS).await?))
    }

    /// What this GPU answers to the clears.
    pub async fn clear_signature(&self) -> Result<Signature, NativeError> {
        let mut passes = Vec::new();
        for clear in &CLEARS {
            passes.push(self.attempt(|| self.clear_pass(clear)).await);
        }
        Ok(Signature::new(WEBGPU_CLEAR, self.pass_answers(&passes).await?))
    }

    /// What this GPU answers to the fixed-function probes.
    pub async fn fixed_function_signature(&self) -> Result<Signature, NativeError> {
        let mut passes = Vec::new();
        for blend in &BLENDS {
            passes.push(self.attempt(|| self.blend_pass(blend)).await);
        }
        for filter in &FILTERS {
            passes.push(self.attempt(|| self.filter_pass(filter)).await);
        }
        for raster in &RASTERS {
            passes.push(self.attempt(|| self.raster_pass(raster)).await);
        }
        Ok(Signature::new(WEBGPU_FIXED_FUNCTION, self.pass_answers(&passes).await?))
    }

    /// What every set in [`EVERY_SET`] answers on this GPU, each beside the interface the registry files it under.
    ///
    /// Each is measured on its own, so one that fails -- the device lost at its readback, say -- leaves the sets before
    /// it answered. A set added to [`EVERY_SET`] fails to compile here until it is measured.
    pub async fn every_set(&self) -> [(&'static str, Result<Signature, NativeError>); EVERY_SET.len()] {
        let [
            split,
            combined,
            clear,
            fixed_function,
            f16_split,
            f16_combined,
            trig_split,
            matrix,
        ] = EVERY_SET;
        [
            (split, self.signature().await),
            (combined, self.combined_signature().await),
            (clear, self.clear_signature().await),
            (fixed_function, self.fixed_function_signature().await),
            (f16_split, self.f16_signature().await),
            (f16_combined, self.f16_combined_signature().await),
            (trig_split, self.trig_signature().await),
            (matrix, self.matrix_signature().await),
        ]
    }

    /// What the adapter and device say the GPU is.
    ///
    /// The first lines are what wgpu reports for every backend. The rest are read from the API the device was opened
    /// through, for whichever of Vulkan, Metal, Direct3D 12 or OpenGL it is: the version numbers there are the device's
    /// own, not anything wgpu infers. Each backend's reader finds nothing when the device was not opened through it.
    #[must_use]
    pub fn report(&self) -> Report {
        let adapter = &self.adapter;
        let info = adapter.get_info();
        let mut report = Report {
            lines: vec![
                format!("name: {}", info.name),
                format!("vendor id: {:#06x}", info.vendor),
                format!("device id: {:#06x}", info.device),
                format!("device type: {:?}", info.device_type),
                format!("pci bus id: {}", info.device_pci_bus_id),
                format!("driver: {}", info.driver),
                format!("driver info: {}", info.driver_info),
                format!("backend: {}", info.backend),
                format!(
                    "subgroup sizes: {}..={}",
                    info.subgroup_min_size, info.subgroup_max_size,
                ),
                format!("features: {:?}", adapter.features()),
                format!("downlevel: {:?}", adapter.get_downlevel_capabilities()),
                format!("limits: {:?}", adapter.limits()),
            ],
            versions: Vec::new(),
        };
        report.versions.extend(named_driver(info.driver));
        #[cfg(not(any(target_vendor = "apple", target_arch = "wasm32")))]
        vulkan_report(adapter, &mut report);
        #[cfg(not(any(target_vendor = "apple", target_arch = "wasm32")))]
        gl_report(adapter, &mut report);
        #[cfg(target_vendor = "apple")]
        metal_report(&self.device, &mut report);
        #[cfg(windows)]
        dx12_report(adapter, &mut report);
        report
    }

    /// What the graphics API beneath wgpu says of the GPU's memory, read from Vulkan, Metal or DXGI.
    pub fn api_memory(&self) -> Result<ApiMemory, NoApiMemory> {
        crate::memory::api_memory(&self.adapter, &self.device)
    }

    /// How far allocating on the device goes before it refuses, allocating at most `cap` bytes.
    ///
    /// Everything allocated is freed once the walk ends. Buffers start at the device's largest, which its default
    /// limits make 256 MiB, and halve on each refusal down to [`WALK_RESOLUTION`].
    pub async fn walk_memory(&self, cap: u64) -> Result<Walk, NativeError> {
        let largest = self.device.limits().max_buffer_size;
        // Set once the device is lost. In a page wgpu still calls a lost device's work-done callback, and its error
        // scopes come back empty, so this is what tells an allocation after the loss that it holds nothing.
        let lost = Arc::new(AtomicBool::new(false));
        let losing = Arc::clone(&lost);
        self.device
            .set_device_lost_callback(move |_reason, _message| losing.store(true, Ordering::Relaxed));
        let allocate = |size| crate::memory::allocate(&self.device, &self.queue, &lost, size);
        walk_with(cap, largest, allocate).await
    }

    /// The pass `build` makes, or the device's reason for refusing a texture, pipeline or upload it asked for.
    async fn attempt<Build>(&self, build: Build) -> Result<Pass, wgpu::Error>
    where
        Build: FnOnce() -> Pass,
    {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let pass = build();
        scope.pop().await.map_or(Ok(pass), Err)
    }

    /// The pass that clears a target of the clear's format to its value, with nothing drawn.
    fn clear_pass(&self, clear: &Clear) -> Pass {
        Pass {
            draw: None,
            load: wgpu::LoadOp::Clear(wgpu::Color {
                r: clear.value,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            }),
            read: Read::Red(clear.format),
            target: self.target(texture_format(clear.format), 1),
        }
    }

    /// The pass that blends the probe's source into a target already holding its destination.
    fn blend_pass(&self, blend: &Blend) -> Pass {
        let format = texture_format(blend.format);
        let target = self.target(format, 1);
        self.upload(&target, &blend.format.texel(blend.destination));
        let component = blend_component(blend.operation);
        let state = wgpu::BlendState {
            color: component,
            alpha: component,
        };
        Pass {
            draw: Some((self.render_pipeline(&blend.shader(), format, Some(state)), None)),
            load: wgpu::LoadOp::Load,
            read: Read::Red(blend.format),
            target,
        }
    }

    /// The pass that samples the probe's two-texel texture into a one-texel `r32float` target.
    fn filter_pass(&self, filter: &Filter) -> Pass {
        let usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
        let source = self.texture(texture_format(filter.format), 2, usage);
        let texels: Vec<u8> = filter
            .texels
            .iter()
            .flat_map(|code| filter.format.texel(*code))
            .collect();
        self.upload(&source, &texels);
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        let pipeline = self.render_pipeline(&filter.shader(), FILTER_TARGET, None);
        let view = source.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.bind_two(
            &pipeline.get_bind_group_layout(0),
            wgpu::BindingResource::TextureView(&view),
            wgpu::BindingResource::Sampler(&sampler),
        );
        Pass {
            draw: Some((pipeline, Some(bind_group))),
            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            read: Read::Word,
            target: self.target(FILTER_TARGET, 1),
        }
    }

    /// The pass that draws the probe's triangle across a cleared target [`RASTER_WIDTH`] texels wide.
    fn raster_pass(&self, raster: &Raster) -> Pass {
        let format = texture_format(RASTER_FORMAT);
        Pass {
            draw: Some((self.render_pipeline(&raster.shader(), format, None), None)),
            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            read: Read::Coverage,
            target: self.target(format, RASTER_WIDTH),
        }
    }

    /// A texture of `format`, `width` texels by one, for the uses `usage` names.
    fn texture(&self, format: wgpu::TextureFormat, width: u32, usage: wgpu::TextureUsages) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    }

    /// A target of `format`, `width` texels by one, that a pass draws into and is copied back from.
    fn target(&self, format: wgpu::TextureFormat, width: u32) -> wgpu::Texture {
        let usage =
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST;
        self.texture(format, width, usage)
    }

    /// Upload `texels`, one row of them, to the whole of `texture`.
    fn upload(&self, texture: &wgpu::Texture, texels: &[u8]) {
        self.queue.write_texture(
            texture.as_image_copy(),
            texels,
            wgpu::TexelCopyBufferLayout::default(),
            texture.size(),
        );
    }

    /// The render pipeline drawing the shader `wgsl` into a target of `format`, blending with `blend`.
    fn render_pipeline(
        &self,
        wgsl: &str,
        format: wgpu::TextureFormat,
        blend: Option<wgpu::BlendState>,
    ) -> wgpu::RenderPipeline {
        let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(wgsl.into()),
        });
        self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vertex"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fragment"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    }

    /// A buffer of `size` bytes, for the uses `usage` names.
    fn buffer(&self, size: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage,
            mapped_at_creation: false,
        })
    }

    /// A bind group of `layout` holding `first` at binding 0 and `second` at binding 1.
    fn bind_two(
        &self,
        layout: &wgpu::BindGroupLayout,
        first: wgpu::BindingResource<'_>,
        second: wgpu::BindingResource<'_>,
    ) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: first,
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: second,
                },
            ],
        })
    }

    /// Submit `encoder`'s commands, wait for them, and give back the bytes they left in `readback`.
    ///
    /// Natively the poll runs the mapping callback before it returns; in a page the browser runs it later, and the wait
    /// is on the channel the callback answers through.
    async fn submit_and_read(
        &self,
        encoder: wgpu::CommandEncoder,
        readback: &wgpu::Buffer,
    ) -> Result<Vec<u8>, NativeError> {
        let submission: wgpu::SubmissionIndex = self.queue.submit([encoder.finish()]);
        let (mapped, outcome) = futures_channel::oneshot::channel::<Result<(), wgpu::BufferAsyncError>>();
        readback.map_async(wgpu::MapMode::Read, .., move |result| {
            // The receiver is awaited below, so it is still there to take this unless the whole read was abandoned.
            let _sent: Result<(), Result<(), wgpu::BufferAsyncError>> = mapped.send(result);
        });
        // Unbounded, this wait never returned on the bench workflow's macos-26-intel lane, where the probe got no
        // answer from the Apple Paravirtual device:
        //
        //     Benchmarking gpus/signature/Apple Paravirtual device (metal): Warming up for 3.0000 s
        //     ##[error]The action 'Benchmark' has timed out after 40 minutes.
        //
        // on commit https://github.com/edge-toolkit/core/commit/ad5e3284e616f20b91f7cc2f1d4cc3d26039a89c at
        // https://github.com/edge-toolkit/core/actions/runs/44185724160/job/134401605307. The same step on that lane
        // later ran past its own timeout and left no log, the job ending in
        //
        // The hosted runner lost communication with the server.
        //
        // on commit https://github.com/edge-toolkit/core/commit/9b0c23ecc64c3de8a0215a557749260975bf3dda at
        // https://github.com/edge-toolkit/core/actions/runs/44840432688/job/136647262861.
        let _status: wgpu::PollStatus = self.device.poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(SUBMISSION_TIMEOUT),
        })?;
        outcome.await??;
        Ok(readback.get_mapped_range(..).to_vec())
    }

    /// What this GPU answers to `passes`, all of them run in one submission, each copied to a slot of its own.
    ///
    /// A pass the device refused answers with the refusal.
    #[expect(
        clippy::little_endian_bytes,
        reason = "WebGPU lays a texel out in little-endian order on every platform"
    )]
    async fn pass_answers(&self, passes: &[Result<Pass, wgpu::Error>]) -> Result<Vec<Answer>, NativeError> {
        let slots = u64::try_from(passes.len()).unwrap_or(u64::MAX);
        let readback = self.buffer(
            slots.saturating_mul(SLOT_BYTES),
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        );
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        // A refused pass keeps its slot but encodes nothing; it answers with its refusal below. Every pass is accepted
        // on the software renderer the coverage lane runs on, so the refusal is filtered out rather than branched on.
        let accepted = (0_u64..)
            .zip(passes)
            .filter_map(|(slot, pass)| Some(slot).zip(pass.as_ref().ok()));
        for (slot, pass) in accepted {
            encode(&mut encoder, pass, &readback, slot.saturating_mul(SLOT_BYTES));
        }
        let held = self.submit_and_read(encoder, &readback).await?;
        let slot_bytes = usize::try_from(SLOT_BYTES).unwrap_or(usize::MAX);
        Ok(passes
            .iter()
            .zip(held.chunks(slot_bytes))
            .map(|(pass, bytes)| {
                let word = bytes.first_chunk::<4>().copied().unwrap_or_default();
                pass.as_ref().map_or_else(
                    |refusal| Answer::Refused(refusal.to_string()),
                    |pass| {
                        Answer::Bits(match pass.read {
                            Read::Red(format) => format.red(word),
                            Read::Word => u32::from_le_bytes(word),
                            Read::Coverage => coverage(bytes),
                        })
                    },
                )
            })
            .collect())
    }

    /// What this GPU answers to `stages`, all of them run in one submission.
    ///
    /// A stage is a shader as the device holds it, beside how many probes it evaluates: each of the probe set's shaders
    /// evaluates one, and the combined shader all of them. Every stage reads `inputs`. A stage the device refused
    /// answers each of its probes with the refusal.
    #[expect(
        clippy::host_endian_bytes,
        reason = "a GPU buffer holds its values in the machine's own byte order"
    )]
    async fn answers(&self, stages: &[(&Probe, u64)], inputs: &[f32; 8]) -> Result<Vec<Answer>, NativeError> {
        let input = self.buffer(INPUT_BYTES, wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST);
        let input_bytes: Vec<u8> = inputs.iter().flat_map(|value| value.to_ne_bytes()).collect();
        self.queue.write_buffer(&input, 0, &input_bytes);
        let readback = self.buffer(
            SIGNATURE_BYTES,
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        );
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        // Each stage's answers sit at an offset of their own, a refused stage's left unwritten; it answers with its
        // refusal below. Every stage compiles on the software renderer the coverage lane runs on, so the refusal is
        // filtered out rather than branched on.
        let mut offset = 0_u64;
        let placed: Vec<_> = stages
            .iter()
            .map(|(probe, probes)| {
                let (bytes, start) = (probes.saturating_mul(OUTPUT_BYTES), offset);
                offset = offset.saturating_add(bytes);
                (probe, bytes, start)
            })
            .collect();
        let compiled = placed
            .into_iter()
            .filter_map(|(probe, bytes, start)| probe.as_ref().ok().map(|pipeline| (pipeline, bytes, start)));
        for (pipeline, bytes, start) in compiled {
            let output = self.buffer(bytes, wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC);
            self.run(&mut encoder, pipeline, &input, &output);
            encoder.copy_buffer_to_buffer(&output, 0, &readback, start, bytes);
        }
        let held = self.submit_and_read(encoder, &readback).await?;
        let mut words = held.as_chunks::<4>().0.iter();
        let mut answers = Vec::new();
        for (probe, probes) in stages {
            for _probe in 0..*probes {
                let word = words.next().copied().unwrap_or_default();
                answers.push(probe.as_ref().map_or_else(
                    |refusal| Answer::Refused(refusal.to_string()),
                    |_pipeline| Answer::Bits(u32::from_ne_bytes(word)),
                ));
            }
        }
        Ok(answers)
    }

    /// Add to `encoder` a run of `pipeline`, reading `input` and leaving what it writes in `output`.
    fn run(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::ComputePipeline,
        input: &wgpu::Buffer,
        output: &wgpu::Buffer,
    ) {
        let bind_group = self.bind_two(
            &pipeline.get_bind_group_layout(0),
            input.as_entire_binding(),
            output.as_entire_binding(),
        );
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
}

/// The wgpu format `format` is.
const fn texture_format(format: Format) -> wgpu::TextureFormat {
    match format {
        Format::Rgba8Unorm => wgpu::TextureFormat::Rgba8Unorm,
        Format::Rgba8UnormSrgb => wgpu::TextureFormat::Rgba8UnormSrgb,
        Format::Rgb10a2Unorm => wgpu::TextureFormat::Rgb10a2Unorm,
        Format::Rg11b10Ufloat => wgpu::TextureFormat::Rg11b10Ufloat,
        Format::Rgba16Float => wgpu::TextureFormat::Rgba16Float,
    }
}

/// The factors and operation that make the blend `operation`, the same for colour and alpha.
const fn blend_component(operation: Operation) -> wgpu::BlendComponent {
    let (src_factor, dst_factor) = match operation {
        Operation::Multiply => (wgpu::BlendFactor::Dst, wgpu::BlendFactor::Zero),
        Operation::Add => (wgpu::BlendFactor::One, wgpu::BlendFactor::One),
    };
    wgpu::BlendComponent {
        src_factor,
        dst_factor,
        operation: wgpu::BlendOperation::Add,
    }
}

impl Compute {
    /// Every shader probe set, compiled for `device`, whose adapter's cooperative matrices are `matrices`.
    ///
    /// A matrix probe of a shape and types the adapter does not offer, or where no subgroup can be 32 lanes, is refused
    /// without being compiled. Compiled anyway, it can run and answer nonsense rather than be refused: NVIDIA's Vulkan
    /// driver, which offers no 8x8x8 `f32` configuration, answered 0 to every such probe on a GB10 and an RTX A1000,
    /// even with C = 1. Or it fails as an internal error, which no validation scope catches, so wgpu panics: Metal did
    /// for a 16x16 probe on an Apple M5 Pro, with `Internal error: Metal: program_source:30:8: error: no type named
    /// 'simdgroup_half16x16' in namespace 'metal'`.
    async fn compile(device: &wgpu::Device, matrices: &Matrices) -> Self {
        let mut probes = Vec::new();
        for shader in shaders() {
            probes.push(compile(device, &shader.1).await);
        }
        let mut f16_probes = Vec::new();
        for shader in f16_shaders() {
            f16_probes.push(compile(device, &shader.1).await);
        }
        let mut trig_probes = Vec::new();
        for shader in trig_shaders() {
            trig_probes.push(compile(device, &shader.1).await);
        }
        let mut matrix_probes = Vec::new();
        for (probe, shader) in MATRIX_PROBES.iter().zip(matrix_shaders()) {
            let compiled = match ensure_offered(probe, matrices) {
                Ok(()) => compile(device, &shader.1).await,
                Err(refusal) => Err(refusal),
            };
            matrix_probes.push(compiled);
        }
        Self {
            combined: compile(device, &combined_shader()).await,
            f16_combined: compile(device, &f16_combined_shader()).await,
            f16_probes,
            matrix_probes,
            probes,
            trig_probes,
        }
    }
}

/// What an adapter gives the matrix probes to run on.
struct Matrices {
    /// The fewest and most lanes the adapter's subgroups have.
    lanes: [u32; 2],
    /// Every cooperative-matrix configuration the adapter offers.
    offered: Vec<wgpu::CooperativeMatrixProperties>,
}

/// `description` as a refusal.
fn refusal(description: String) -> wgpu::Error {
    wgpu::Error::Validation {
        source: Box::new(std::io::Error::other(description.clone())),
        description,
    }
}

/// Refuse `probe` where the subgroups are not all 32 lanes, or no configuration offered has its shape and types.
///
/// Every matrix shader is one workgroup of 32 invocations sharing one matrix, so it is one subgroup only where the
/// device's subgroups are 32 lanes wide. Narrower, several subgroups would each store the same tile; wider, Vulkan
/// leaves a subgroup-scope cooperative matrix in a workgroup that is not a multiple of the subgroup undefined. A range
/// that merely includes 32, such as 16..=32, could run either, so it is refused too.
fn ensure_offered(probe: &MatrixProbe, matrices: &Matrices) -> Result<(), wgpu::Error> {
    let [fewest, most] = matrices.lanes;
    let size = match probe.shape {
        Shape::F16x16 => 16,
        Shape::F32x8 => 8,
    };
    let operand = match probe.shape {
        Shape::F16x16 => wgpu::CooperativeScalarType::F16,
        Shape::F32x8 => wgpu::CooperativeScalarType::F32,
    };
    let accumulator = match probe.accumulator {
        Accumulator::F16 => wgpu::CooperativeScalarType::F16,
        Accumulator::F32 => wgpu::CooperativeScalarType::F32,
    };
    // The checks are values rather than branches: the runners that measure coverage offer cooperative matrices only on
    // subgroups narrower than 32, so no branch that lets a probe through could be taken there.
    let wanted = ([size; 3], operand, accumulator);
    let fits = |properties: &wgpu::CooperativeMatrixProperties| {
        let offered = [properties.m_size, properties.n_size, properties.k_size];
        (offered, properties.ab_type, properties.cr_type) == wanted
    };
    let narrow = ([fewest, most] != [32, 32])
        .then(|| format!("subgroups of {fewest}..={most} lanes, where a matrix probe needs 32"));
    let unoffered = (!matrices.offered.iter().any(fits))
        .then(|| format!("no {size}x{size}x{size} cooperative matrix of {operand:?} into {accumulator:?} offered"));
    narrow
        .or(unoffered)
        .map_or(Ok(()), |description| Err(refusal(description)))
}

/// The pipeline on `device` that runs the shader `wgsl` holds.
///
/// A shader the device will not compile, or a pipeline it will not build, is a refusal, which the error scope catches.
async fn compile(device: &wgpu::Device, wgsl: &str) -> Probe {
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(wgsl.into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: None,
        module: &module,
        entry_point: Some("main"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    });
    scope.pop().await.map_or(Ok(pipeline), Err)
}

/// Add to `encoder` `pass`, and a copy of its target to `readback` at `offset`.
fn encode(encoder: &mut wgpu::CommandEncoder, pass: &Pass, readback: &wgpu::Buffer, offset: u64) {
    let view = pass.target.create_view(&wgpu::TextureViewDescriptor::default());
    {
        let mut render = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: pass.load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..wgpu::RenderPassDescriptor::default()
        });
        if let Some((pipeline, bind_group)) = &pass.draw {
            render.set_pipeline(pipeline);
            if let Some(bind_group) = bind_group {
                render.set_bind_group(0, bind_group, &[]);
            }
            render.draw(0..3, 0..1);
        }
    }
    encoder.copy_texture_to_buffer(
        pass.target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset,
                bytes_per_row: None,
                rows_per_image: None,
            },
        },
        pass.target.size(),
    );
}

/// `version`, a Vulkan driver version reported by the vendor with the PCI ID `vendor`, in that vendor's encoding.
///
/// Vulkan leaves the encoding to the vendor. NVIDIA's packs four fields, Intel's on Windows -- which `on_windows` says
/// the driver is running on -- two, and every other driver on record uses the API version's own layout.
#[cfg(not(target_vendor = "apple"))]
#[must_use]
pub fn vulkan_driver_version(vendor: u32, version: u32, on_windows: bool) -> String {
    if vendor == 0x10de {
        return format!(
            "{}.{}.{}.{}",
            version >> 22_u32,
            (version >> 14_u32) & 0xff,
            (version >> 6_u32) & 0xff,
            version & 0x3f,
        );
    }
    if vendor == 0x8086 && on_windows {
        return format!("{}.{}", version >> 14_u32, version & 0x3fff);
    }
    vulkan_api_version(version)
}

/// `version`, a version in the layout Vulkan gives its API versions: variant, major, minor and patch.
#[cfg(not(target_vendor = "apple"))]
#[must_use]
pub fn vulkan_api_version(version: u32) -> String {
    let semantic = format!(
        "{}.{}.{}",
        (version >> 22_u32) & 0x7f,
        (version >> 12_u32) & 0x3ff,
        version & 0xfff,
    );
    match version >> 29_u32 {
        0 => semantic,
        variant => format!("{semantic} (variant {variant})"),
    }
}

/// Add to `report` what Vulkan says the GPU is, when `adapter` was opened through Vulkan.
///
/// wgpu builds its Vulkan backend everywhere but on Apple's platforms and in a page.
#[cfg(not(any(target_vendor = "apple", target_arch = "wasm32")))]
fn vulkan_report(adapter: &wgpu::Adapter, report: &mut Report) {
    // An adapter opened through another API has no Vulkan side, and adds nothing here: a Direct3D 12 one on Windows,
    // and on every platform one on wgpu's no-op backend.
    let Ok(properties) = r#unsafe::with_vulkan(adapter, |vulkan| vulkan.physical_device_capabilities().properties())
    else {
        return;
    };
    let limits = properties.limits;
    let uuid = properties
        .pipeline_cache_uuid
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .concat();
    let name = properties
        .device_name_as_c_str()
        .map_or_else(|error| error.to_string(), |name| name.to_string_lossy().into_owned());
    let api = vulkan_api_version(properties.api_version);
    let driver = vulkan_driver_version(properties.vendor_id, properties.driver_version, cfg!(windows));
    report.lines.extend([
        format!("vulkan device name: {name}"),
        format!("vulkan device type: {:?}", properties.device_type),
        format!("vulkan vendor id: {:#06x}", properties.vendor_id),
        format!("vulkan device id: {:#06x}", properties.device_id),
        format!("vulkan api version: {api}"),
        format!(
            "vulkan driver version: {driver} (raw {:#010x})",
            properties.driver_version,
        ),
        format!("vulkan pipeline cache uuid: {uuid}"),
        format!("vulkan sub-pixel precision bits: {}", limits.sub_pixel_precision_bits),
        format!("vulkan sub-texel precision bits: {}", limits.sub_texel_precision_bits),
        format!("vulkan mipmap precision bits: {}", limits.mipmap_precision_bits),
        format!("vulkan viewport sub-pixel bits: {}", limits.viewport_sub_pixel_bits),
    ]);
    report
        .versions
        .extend([("driver_version", driver), ("api_version", format!("Vulkan {api}"))]);
}

/// The name the registry gives `backend`, the graphics API a device was opened through, or `unknown`.
///
/// wgpu's one OpenGL backend opens either OpenGL or OpenGL ES, whichever the platform offers, and only the context's
/// own `version` string says which: OpenGL ES's starts `OpenGL ES`, and OpenGL's with its version number.
#[must_use]
pub fn backend_name(backend: wgpu::Backend, version: &str) -> &'static str {
    if backend == wgpu::Backend::Gl && version.starts_with("OpenGL ES") {
        return "opengl-es";
    }
    BACKENDS
        .iter()
        .find(|known| known.0 == backend)
        .map_or("unknown", |known| known.1)
}

/// The `api_version` and `driver_version` a row records from `version`, the string OpenGL gives for `GL_VERSION`.
///
/// OpenGL's is `<major>.<minor>[.<release>] <vendor-specific information>`, and OpenGL ES's the same after `OpenGL ES
/// `. The number is the API's version, and whatever the vendor put after it is the driver's, kept exactly as given:
/// Mesa writes the profile and its own release there, NVIDIA its driver's version. A string with nothing after the
/// number gives the API's version alone.
#[must_use]
pub fn gl_versions(version: &str) -> Vec<(&'static str, String)> {
    let (api, rest) = version
        .strip_prefix("OpenGL ES ")
        .map_or(("OpenGL", version), |rest| ("OpenGL ES", rest));
    let (number, vendor) = rest.split_once(' ').unwrap_or((rest, ""));
    let mut versions = vec![("api_version", format!("{api} {number}"))];
    if !vendor.trim().is_empty() {
        versions.push(("driver_version", vendor.trim().to_owned()));
    }
    versions
}

/// Whether a device opened through `backend` runs compute shaders.
///
/// That is by its downlevel `flags` and, for OpenGL, the context's own `version` string. wgpu's OpenGL backend claims
/// compute shaders wherever the driver offers them, but translates every shader for the GLSL of the context it was
/// given, and GLSL has compute shaders only from OpenGL 4.3 and OpenGL ES 3.1. A version string this cannot read leaves
/// the flags to decide.
#[must_use]
pub fn runs_compute(backend: wgpu::Backend, flags: wgpu::DownlevelFlags, version: &str) -> bool {
    if !flags.contains(wgpu::DownlevelFlags::COMPUTE_SHADERS) {
        return false;
    }
    if backend != wgpu::Backend::Gl {
        return true;
    }
    // NVIDIA's EGL gives wgpu 29.0.4 an OpenGL 3.3 context, where a compute shader fails as it is translated, which
    // wgpu treats as fatal: Shader translation error for stage ShaderStages(COMPUTE): The selected version doesn't
    // support Features(BUFFER_STORAGE | COMPUTE_SHADER)
    let (minimum, rest) = version
        .strip_prefix("OpenGL ES ")
        .map_or(((4, 3), version), |rest| ((3, 1), rest));
    let mut parts = rest.split([' ', '.']).map(str::parse::<u32>);
    match (parts.next(), parts.next()) {
        (Some(Ok(major)), Some(Ok(minor))) => (major, minor) >= minimum,
        _ => true,
    }
}

/// Whether a device opened through `backend` runs compute shaders, as [`runs_compute`] judges by `flags` and `version`.
///
/// # Errors
///
/// [`NoCompute::Context`], naming the backend and `version`, where it runs none.
pub fn compute_shaders(backend: wgpu::Backend, flags: wgpu::DownlevelFlags, version: &str) -> Result<(), NoCompute> {
    if runs_compute(backend, flags, version) {
        return Ok(());
    }
    Err(NoCompute::Context {
        backend: backend_name(backend, version),
        version: version.to_owned(),
    })
}

/// The `driver` a row records from `driver`, the driver's own name for itself as wgpu reads it.
///
/// That is Vulkan's `driverName`. The other APIs leave it empty, and Vulkan without `VK_KHR_driver_properties` gives
/// `?`, neither of which names anything, so neither is recorded.
#[must_use]
pub fn named_driver(driver: String) -> Vec<(&'static str, String)> {
    if matches!(driver.as_str(), "" | "?") {
        return Vec::new();
    }
    vec![("driver", driver)]
}

/// What OpenGL says it is, from `vendor` and `version`, the strings it gives for `GL_VENDOR` and `GL_VERSION`.
///
/// OpenGL has no query naming its driver. `GL_VENDOR`, the implementation's own name for who provides it, is the
/// nearest it comes, and is recorded as the driver as given; `GL_VERSION` gives the API's and the driver's versions.
#[must_use]
pub fn gl_report_of(vendor: &str, version: &str) -> Report {
    let mut versions = vec![("driver", vendor.to_owned())];
    versions.extend(gl_versions(version));
    Report {
        lines: vec![format!("gl vendor: {vendor}"), format!("gl version: {version}")],
        versions,
    }
}

/// Add what OpenGL itself says to `report`, for an adapter opened through it.
#[cfg(not(any(target_vendor = "apple", target_arch = "wasm32")))]
fn gl_report(adapter: &wgpu::Adapter, report: &mut Report) {
    // An adapter opened through another API has no OpenGL side, and adds nothing here. Reading the side of one that
    // does takes a real OpenGL driver, which no CI runner has, so only the two strings are read: what they say is made
    // into the report by `gl_report_of`, which the tests drive with the strings real drivers give.
    let said = r#unsafe::gl_strings(adapter).map_or_default(|(vendor, version)| gl_report_of(&vendor, &version));
    report.lines.extend(said.lines);
    report.versions.extend(said.versions);
}

/// The name of `architecture`, the one a Metal device gives, or `none` for a device that gives none.
///
/// `None` is what an Intel Mac's GPU answers: Metal names the architecture of Apple's own GPUs alone, and that is no
/// failure of the device, so the report says so rather than leaving the line out.
#[cfg(target_vendor = "apple")]
#[must_use]
pub fn metal_architecture_name(architecture: Option<&objc2_metal::MTLArchitecture>) -> String {
    architecture.map_or_else(|| "none".to_owned(), |named| named.name().to_string())
}

/// Add to `report` what Metal says the GPU is, when `device` was opened through Metal.
#[cfg(target_vendor = "apple")]
fn metal_report(device: &wgpu::Device, report: &mut Report) {
    // A device opened through another API has no Metal side: on Apple's platforms, only one on wgpu's no-op backend.
    let Ok(lines) = r#unsafe::with_metal(device, |metal| metal_lines(metal.raw_device())) else {
        return;
    };
    report.lines.extend(lines);
}

/// What the Metal device `raw` says it is, one `name: value` line each.
#[cfg(target_vendor = "apple")]
fn metal_lines(raw: &objc2::runtime::ProtocolObject<dyn objc2_metal::MTLDevice>) -> Vec<String> {
    let families: Vec<&str> = METAL_FAMILIES
        .iter()
        .filter(|family| raw.supportsFamily(family.0))
        .map(|family| family.1)
        .collect();
    let architecture = r#unsafe::with_metal_architecture(raw, metal_architecture_name);
    vec![
        format!("metal device name: {}", raw.name()),
        format!("metal architecture: {architecture}"),
        format!("metal registry id: {:#x}", raw.registryID()),
        format!("metal location: {:?}", raw.location()),
        format!("metal recommended working set: {}", raw.recommendedMaxWorkingSetSize()),
        format!("metal families: {}", families.join(" ")),
    ]
}

/// Add to `report` what DXGI says the GPU is, when `adapter` was opened through Direct3D 12.
///
/// The user-mode driver version is the one Windows shows for the display driver, four fields of 16 bits each.
#[cfg(windows)]
fn dx12_report(adapter: &wgpu::Adapter, report: &mut Report) {
    let Ok((description, driver)) = r#unsafe::dxgi_adapter(adapter) else {
        return;
    };
    // A version DXGI would not give is left out of `versions`, whose values a registry row records as fact.
    match driver {
        Ok(version) => {
            // DXGI hands the four 16-bit fields over packed in a signed 64-bit integer; its bits are the version.
            let packed = version.cast_unsigned();
            let driver = format!(
                "{}.{}.{}.{}",
                packed >> 48_u32,
                (packed >> 32_u32) & 0xffff,
                (packed >> 16_u32) & 0xffff,
                packed & 0xffff,
            );
            report.lines.push(format!("dxgi driver version: {driver}"));
            report.versions.push(("driver_version", driver));
        }
        Err(error) => report.lines.push(format!("dxgi driver version: {error}")),
    }
    match description {
        Ok(description) => {
            let name = String::from_utf16_lossy(&description.Description);
            report.lines.extend([
                format!("dxgi description: {}", name.trim_end_matches('\0')),
                format!("dxgi vendor id: {:#06x}", description.VendorId),
                format!("dxgi device id: {:#06x}", description.DeviceId),
                format!("dxgi subsystem id: {:#010x}", description.SubSysId),
                format!("dxgi revision: {:#x}", description.Revision),
                format!("dxgi dedicated video memory: {}", description.DedicatedVideoMemory),
            ]);
        }
        Err(error) => report.lines.push(format!("dxgi description: {error}")),
    }
}
