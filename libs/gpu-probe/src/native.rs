//! Puts the probe sets to a GPU through wgpu, and reads what the device says it is.
//!
//! Built natively, wgpu opens the machine's GPUs itself and the device is described from the API it was opened through.
//! Built for `wasm32`, wgpu opens the browser's WebGPU, so the same probes run in a page. Every call that waits on the
//! GPU is async, since a page cannot block for it; a native caller blocks on each with an executor.
//!
//! The device is asked for the one optional feature a clear needs, rendering to `rg11b10ufloat`, when the adapter has
//! it; without it that clear is refused, which its signature records like any other refusal.
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

#[cfg(target_vendor = "apple")]
use objc2_metal::MTLDevice as _;
#[cfg(windows)]
use windows::core::Interface as _;

use crate::{
    Answer, BLENDS, Blend, CLEARS, Clear, FILTERS, Filter, Format, INPUTS, Operation, PROBE_COUNT, RASTER_FORMAT,
    RASTER_WIDTH, RASTERS, Raster, Signature, WEBGPU, WEBGPU_CLEAR, WEBGPU_COMBINED, WEBGPU_FIXED_FUNCTION,
    combined_shader, coverage, shaders,
};

/// How many bytes the storage buffer every probe shader reads [`INPUTS`] from holds.
#[expect(
    clippy::as_conversions,
    reason = "a few dozen bytes fit any u64, and a const has no fallible conversion from usize"
)]
const INPUT_BYTES: u64 = size_of_val(&INPUTS) as u64;

/// How many bytes the one `u32` every probe shader writes takes.
const OUTPUT_BYTES: u64 = 4;

/// How many bytes the readback of a signature's answers holds: one word a probe.
#[expect(
    clippy::as_conversions,
    reason = "a few dozen probes fit any u64, and a const has no fallible conversion from usize"
)]
const SIGNATURE_BYTES: u64 = PROBE_COUNT as u64 * OUTPUT_BYTES;

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
    /// Waiting for the device to finish the probes failed.
    #[error("the GPU did not finish the probes: {0}")]
    Poll(#[from] wgpu::PollError),
    /// The buffer the answers were copied to could not be read.
    #[error("the GPU's answers could not be read back: {0}")]
    Readback(#[from] wgpu::BufferAsyncError),
    /// wgpu dropped the buffer's mapping callback without ever calling it.
    #[error("the GPU never said whether its answers could be read back: {0}")]
    Unmapped(#[from] futures_channel::oneshot::Canceled),
}

/// A probe as a device holds it: the pipeline that evaluates it, or the device's reason for refusing to build one.
type Probe = Result<wgpu::ComputePipeline, wgpu::Error>;

/// What a device says it is, read from wgpu and from the API beneath it.
#[derive(Debug)]
#[non_exhaustive]
pub struct Report {
    /// Every value, one `name: value` line each.
    pub lines: Vec<String>,
    /// The values a registry row has a field for: `driver_version` and `api_version`, as far as the API gives them.
    pub versions: Vec<(&'static str, String)>,
}

/// This machine's GPU, opened through wgpu, with the probe set compiled for it.
pub struct Gpu {
    /// The adapter the device was opened on.
    adapter: wgpu::Adapter,
    /// The whole probe set as one shader.
    combined: Probe,
    /// The device the probes run on.
    device: wgpu::Device,
    /// The probe set, in output order.
    probes: Vec<Probe>,
    /// Where the device's work is submitted.
    queue: wgpu::Queue,
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

    /// Open every adapter wgpu finds through Vulkan, Metal and Direct3D 12, with the probe set compiled for each.
    ///
    /// They come in the order wgpu lists them. A software renderer, such as llvmpipe or WARP, is opened beside the
    /// GPUs: it answers as the CPU's own arithmetic does through that renderer, which is a signature of its own. So is
    /// a GPU reached through two of those APIs, which is opened once through each.
    ///
    /// Each adapter gives its own outcome, so one whose device will not open -- a broken driver, or a software renderer
    /// that rejects the default limits -- stops none of the others being measured.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn open_all() -> Vec<Result<Self, NativeError>> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut gpus = Vec::new();
        for adapter in instance.enumerate_adapters(wgpu::Backends::PRIMARY).await {
            gpus.push(Self::open_adapter(adapter).await);
        }
        gpus
    }

    /// Open a device on `adapter`, and compile the probe set for it.
    ///
    /// For a caller that chose the adapter itself, from an instance of its own: one limited to a backend, say.
    pub async fn open_adapter(adapter: wgpu::Adapter) -> Result<Self, NativeError> {
        let descriptor = wgpu::DeviceDescriptor {
            required_features: adapter.features() & wgpu::Features::RG11B10UFLOAT_RENDERABLE,
            ..wgpu::DeviceDescriptor::default()
        };
        let (device, queue) = adapter.request_device(&descriptor).await?;
        let mut probes = Vec::new();
        for shader in shaders() {
            probes.push(compile(&device, &shader.1).await);
        }
        Ok(Self {
            combined: compile(&device, &combined_shader()).await,
            probes,
            adapter,
            device,
            queue,
        })
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

    /// The name the registry gives the graphics API the device was opened through, or `unknown`.
    #[must_use]
    pub fn backend(&self) -> &'static str {
        let backend = self.info().backend;
        BACKENDS
            .iter()
            .find(|known| known.0 == backend)
            .map_or("unknown", |known| known.1)
    }

    /// What the probe set answers on this GPU, each probe in a shader of its own.
    pub async fn signature(&self) -> Result<Signature, NativeError> {
        let stages: Vec<(&Probe, u64)> = self.probes.iter().map(|probe| (probe, 1)).collect();
        Ok(Signature::new(WEBGPU, self.answers(&stages).await?))
    }

    /// What the probe set answers on this GPU, every probe in the one combined shader.
    pub async fn combined_signature(&self) -> Result<Signature, NativeError> {
        let words = u64::try_from(PROBE_COUNT).unwrap_or(u64::MAX);
        let answers = self.answers(&[(&self.combined, words)]).await?;
        Ok(Signature::new(WEBGPU_COMBINED, answers))
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

    /// What the adapter and device say the GPU is.
    ///
    /// The first lines are what wgpu reports for every backend. The rest are read from the API the device was opened
    /// through, for whichever of Vulkan, Metal or Direct3D 12 it is: the version numbers there are the device's own,
    /// not anything wgpu infers. Each backend's reader finds nothing when the device was not opened through it.
    #[must_use]
    pub fn report(&self) -> Report {
        let adapter = &self.adapter;
        let info = adapter.get_info();
        #[cfg_attr(
            target_arch = "wasm32",
            expect(
                unused_mut,
                reason = "in a page there is no API beneath wgpu to add to the report from"
            )
        )]
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
                    info.subgroup_min_size, info.subgroup_max_size
                ),
                format!("features: {:?}", adapter.features()),
                format!("downlevel: {:?}", adapter.get_downlevel_capabilities()),
                format!("limits: {:?}", adapter.limits()),
            ],
            versions: Vec::new(),
        };
        #[cfg(not(any(target_vendor = "apple", target_arch = "wasm32")))]
        vulkan_report(adapter, &mut report);
        #[cfg(target_vendor = "apple")]
        metal_report(&self.device, &mut report);
        #[cfg(windows)]
        dx12_report(adapter, &mut report);
        report
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
        let _submission: wgpu::SubmissionIndex = self.queue.submit([encoder.finish()]);
        let (mapped, outcome) = futures_channel::oneshot::channel::<Result<(), wgpu::BufferAsyncError>>();
        readback.map_async(wgpu::MapMode::Read, .., move |result| {
            // The receiver is awaited below, so it is still there to take this unless the whole read was abandoned.
            let _sent: Result<(), Result<(), wgpu::BufferAsyncError>> = mapped.send(result);
        });
        let _status: wgpu::PollStatus = self.device.poll(wgpu::PollType::wait_indefinitely())?;
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
    /// evaluates one, and the combined shader all of them. A stage the device refused answers each of its probes with
    /// the refusal.
    #[expect(
        clippy::host_endian_bytes,
        reason = "a GPU buffer holds its values in the machine's own byte order"
    )]
    async fn answers(&self, stages: &[(&Probe, u64)]) -> Result<Vec<Answer>, NativeError> {
        let input = self.buffer(INPUT_BYTES, wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST);
        let input_bytes: Vec<u8> = INPUTS.iter().flat_map(|value| value.to_ne_bytes()).collect();
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
            version & 0x3f
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
        version & 0xfff
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
#[expect(
    unsafe_code,
    reason = "wgpu hands out the Vulkan adapter beneath it only through an unsafe call"
)]
fn vulkan_report(adapter: &wgpu::Adapter, report: &mut Report) {
    // SAFETY: the Vulkan adapter is only read from, and is not kept past this function.
    let hal = unsafe { adapter.as_hal::<wgpu::hal::api::Vulkan>() };
    // An adapter opened through another API has no Vulkan side, and adds nothing here: a Direct3D 12 one on Windows,
    // and on every platform one on wgpu's no-op backend.
    let Some(vulkan) = hal else { return };
    let properties = vulkan.physical_device_capabilities().properties();
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
            properties.driver_version
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

/// Add to `report` what Metal says the GPU is, when `device` was opened through Metal.
#[cfg(target_vendor = "apple")]
#[expect(
    unsafe_code,
    reason = "wgpu hands out the Metal device beneath it only through an unsafe call"
)]
fn metal_report(device: &wgpu::Device, report: &mut Report) {
    // SAFETY: the Metal device is only read from, and is not kept past this function.
    let hal = unsafe { device.as_hal::<wgpu::hal::api::Metal>() };
    // A device opened through another API has no Metal side: on Apple's platforms, only one on wgpu's no-op backend.
    let Some(metal) = hal else { return };
    let raw = metal.raw_device();
    let families: Vec<&str> = METAL_FAMILIES
        .iter()
        .filter(|family| raw.supportsFamily(family.0))
        .map(|family| family.1)
        .collect();
    report.lines.extend([
        format!("metal device name: {}", raw.name()),
        format!("metal architecture: {}", raw.architecture().name()),
        format!("metal registry id: {:#x}", raw.registryID()),
        format!("metal location: {:?}", raw.location()),
        format!("metal recommended working set: {}", raw.recommendedMaxWorkingSetSize()),
        format!("metal families: {}", families.join(" ")),
    ]);
}

/// Add to `report` what DXGI says the GPU is, when `adapter` was opened through Direct3D 12.
///
/// The user-mode driver version is the one Windows shows for the display driver, four fields of 16 bits each.
#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "wgpu hands out the DXGI adapter beneath it only through an unsafe call, and DXGI's calls are unsafe"
)]
fn dx12_report(adapter: &wgpu::Adapter, report: &mut Report) {
    // SAFETY: the DXGI adapter is only read from, and is not kept past this function.
    let hal = unsafe { adapter.as_hal::<wgpu::hal::api::Dx12>() };
    let Some(dx12) = hal else { return };
    let raw = dx12.raw_adapter();
    // SAFETY: the call only fills in the description it returns, from an adapter that outlives it.
    let description = unsafe { raw.GetDesc2() };
    // SAFETY: the call only fills in the version it returns, from an adapter that outlives it.
    let driver = unsafe { raw.CheckInterfaceSupport(&windows::Win32::Graphics::Dxgi::IDXGIDevice::IID) };
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
                packed & 0xffff
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
