//! Puts the probe set to this machine with no browser involved, to its GPU through wgpu and to its CPU.
//!
//! Each signature is checked against the one recorded for that hardware. A machine nobody has recorded still has to
//! answer every probe legally, and the same way twice; what it answered is printed, which is how its signature comes to
//! be recorded.
#![cfg(test)]

use std::sync::LazyLock;

use et_gpu_probe::{
    Answer, HOST, INPUTS, PROBE_COUNT, Signature, WEBGPU, WEBGPU_COMBINED, combined_shader, host, shaders,
};
use futures_executor::block_on;

/// How many bytes the storage buffer every probe shader reads [`INPUTS`] from holds.
const INPUT_BYTES: u64 = 32;

/// How many bytes the one `u32` every probe shader writes takes.
const OUTPUT_BYTES: u64 = 4;

/// How many bytes the answers to the whole probe set take.
const SIGNATURE_BYTES: u64 = 64;

/// How many words the answers to the whole probe set take, one a probe.
const PROBE_WORDS: u64 = 16;

/// This machine's GPU, opened once and with the probe set compiled for it.
static GPU: LazyLock<Gpu> = LazyLock::new(|| {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
    let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
    let info = adapter.get_info();
    Gpu {
        hardware: format!(
            "{} | {} {} | {}",
            info.name, info.driver, info.driver_info, info.backend
        ),
        combined: compile(&device, &combined_shader()),
        probes: shaders().iter().map(|shader| compile(&device, &shader.1)).collect(),
        vendor: info.vendor,
        device,
        queue,
    }
});

/// A probe as a device holds it: the pipeline that evaluates it, or the device's reason for refusing to build one.
type Probe = Result<wgpu::ComputePipeline, wgpu::Error>;

/// A GPU the probes are put to.
struct Gpu {
    /// The whole probe set as one shader.
    combined: Probe,
    /// The device the probes run on.
    device: wgpu::Device,
    /// What the adapter behind the device reports itself as: its name, driver and backend.
    hardware: String,
    /// The probe set, in output order.
    probes: Vec<Probe>,
    /// Where the device's work is submitted.
    queue: wgpu::Queue,
    /// The PCI vendor ID the adapter reports, which is none on some backends.
    vendor: u32,
}

/// Print `signature` beside the `hardware` it was measured on through `interface`.
///
/// Hardware the registry does not hold yet has no label to print, and its fingerprint is what gets it one.
fn report(interface: &str, hardware: &str, signature: &Signature) {
    println!("{interface} hardware: {hardware}");
    println!("{interface} fingerprint: {}", signature.fingerprint());
    println!("{interface} registry: {}", signature.identify().join(" | "));
    println!("{interface} traits: {}", signature.traits());
    for line in signature.describe() {
        println!("{interface} {line}");
    }
}

/// A buffer of `size` bytes on `device`, for the uses `usage` names.
fn buffer(device: &wgpu::Device, size: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size,
        usage,
        mapped_at_creation: false,
    })
}

/// The pipeline on `device` that runs the shader `wgsl` holds.
///
/// A shader the device will not compile, or a pipeline it will not build, is a refusal, which the error scope catches.
fn compile(device: &wgpu::Device, wgsl: &str) -> Probe {
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
    block_on(scope.pop()).map_or(Ok(pipeline), Err)
}

/// Add to `encoder` a run of `pipeline` on `device`, reading `input` and leaving what it writes in `output`.
#[expect(
    clippy::single_call_fn,
    reason = "one stage's run, kept apart from the loop over the stages"
)]
fn run(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &wgpu::ComputePipeline,
    input: &wgpu::Buffer,
    output: &wgpu::Buffer,
) {
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: input.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: output.as_entire_binding(),
            },
        ],
    });
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, &bind_group, &[]);
    pass.dispatch_workgroups(1, 1, 1);
}

/// What this machine's GPU answers to `stages`, all of them run in one submission.
///
/// A stage is a shader as the device holds it, beside how many probes it evaluates: each of the probe set's shaders
/// evaluates one, and the combined shader all of them. A stage the device refused answers each of its probes with
/// the refusal.
#[expect(
    clippy::host_endian_bytes,
    reason = "a GPU buffer holds its values in the machine's own byte order"
)]
fn gpu_answers(stages: &[(&Probe, u64)]) -> Vec<Answer> {
    let Gpu { device, queue, .. } = &*GPU;
    let input = buffer(
        device,
        INPUT_BYTES,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    );
    let input_bytes: Vec<u8> = INPUTS.iter().flat_map(|value| value.to_ne_bytes()).collect();
    queue.write_buffer(&input, 0, &input_bytes);
    let readback = buffer(
        device,
        SIGNATURE_BYTES,
        wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
    );
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let mut offset = 0_u64;
    for (probe, probes) in stages {
        let bytes = probes.saturating_mul(OUTPUT_BYTES);
        if let Ok(pipeline) = probe {
            let output = buffer(
                device,
                bytes,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            );
            run(device, &mut encoder, pipeline, &input, &output);
            encoder.copy_buffer_to_buffer(&output, 0, &readback, offset, bytes);
        }
        offset = offset.saturating_add(bytes);
    }
    let _submission: wgpu::SubmissionIndex = queue.submit([encoder.finish()]);
    readback.map_async(wgpu::MapMode::Read, .., |mapped| mapped.unwrap());
    let _status: wgpu::PollStatus = device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let held = readback.get_mapped_range(..);
    let mut words = held.as_chunks::<4>().0.iter();
    let mut answers = Vec::new();
    for (probe, probes) in stages {
        for _probe in 0..*probes {
            let word = words.next().unwrap();
            answers.push(match probe {
                Ok(_) => Answer::Bits(u32::from_ne_bytes(*word)),
                Err(refusal) => Answer::Refused(refusal.to_string()),
            });
        }
    }
    answers
}

/// What this machine's GPU answers to the probe set, each probe in a shader of its own.
fn gpu_signature() -> Signature {
    let stages: Vec<(&Probe, u64)> = GPU.probes.iter().map(|probe| (probe, 1)).collect();
    Signature::new(WEBGPU, gpu_answers(&stages))
}

/// What this machine's GPU answers to the probe set, every probe in the one combined shader.
fn combined_signature() -> Signature {
    Signature::new(WEBGPU_COMBINED, gpu_answers(&[(&GPU.combined, PROBE_WORDS)]))
}

/// What this machine's CPU answers to the probe set.
fn cpu_signature() -> Signature {
    Signature::new(HOST, host().into_iter().map(Answer::Bits).collect())
}

#[test]
fn this_gpu_answers_every_probe_legally_and_the_same_way_twice() {
    let signature = gpu_signature();
    report("wgpu", &GPU.hardware, &signature);
    println!("wgpu vendor id: {:#06x}", GPU.vendor);

    assert_eq!(signature.completed(), PROBE_COUNT);
    assert!(!signature.traits().contains("=other"));
    assert_eq!(signature.fingerprint(), gpu_signature().fingerprint());
}

#[test]
fn this_gpu_answers_the_combined_shader_legally_and_the_same_way_twice() {
    let signature = combined_signature();
    report("wgpu combined", &GPU.hardware, &signature);

    assert_eq!(signature.completed(), PROBE_COUNT);
    assert!(!signature.traits().contains("=other"));
    assert_eq!(signature.fingerprint(), combined_signature().fingerprint());
}

// The platforms whose GPU and CPU each have a signature on record: an Apple GPU through Metal, llvmpipe through
// Vulkan and the Basic Render Driver through Direct3D 12. A machine of one of these platforms with hardware not yet
// on record fails here, and the fingerprint the tests beside this one print is what gets it recorded.
#[cfg(any(
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "windows", target_arch = "x86_64"),
))]
#[test]
fn the_registry_has_seen_this_gpu_and_this_cpu_on_this_platform() {
    // An adapter that reports no vendor ID, as Metal's does, is held to its platform alone.
    let vendor = et_gpu_probe::GpuVendor::from_id(GPU.vendor);
    let gpu_seen = gpu_signature().seen().unwrap();
    assert!(gpu_seen.iter().any(|sighting| {
        sighting.is_here() && (vendor == et_gpu_probe::GpuVendor::Unknown || sighting.gpu == vendor)
    }));
    let cpu_seen = cpu_signature().seen().unwrap();
    assert!(cpu_seen.iter().any(et_gpu_probe::Sighting::is_here));
}

#[test]
fn this_cpu_answers_every_probe_the_same_way_twice() {
    let signature = cpu_signature();
    let hardware = format!("{} {}", std::env::consts::OS, std::env::consts::ARCH);
    report(HOST, &hardware, &signature);

    assert_eq!(signature.fingerprint(), cpu_signature().fingerprint());
}
