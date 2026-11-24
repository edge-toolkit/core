//! Puts the probe sets to this machine with no browser involved, to its GPU through wgpu and to its CPU.
//!
//! Each shader signature is checked against the one recorded for that hardware. A machine nobody has recorded still has
//! to answer every probe legally, and the same way twice; what it answered is printed, which is how its signature comes
//! to be recorded. The clears and fixed-function probes, which no shader compiler decides, have to be answered the same
//! way twice. Beside the signatures the test prints what the device says it is, read from the API it was opened through
//! rather than from what wgpu makes of it.
#![cfg(test)]

use std::sync::LazyLock;

use et_gpu_probe::native::{Gpu, NativeError, NoCompute, backend_name, gl_versions, runs_compute};
use et_gpu_probe::{Answer, BLENDS, CLEARS, FILTERS, HOST, PROBE_COUNT, RASTERS, Signature, host};
use futures_executor::block_on;
use pretty_assertions::assert_eq;
use pretty_assertions::assert_ne;

/// This machine's GPU, opened once and with the probe set compiled for it.
static GPU: LazyLock<Gpu> = LazyLock::new(|| block_on(Gpu::open()).unwrap());

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

/// What this machine's CPU answers to the probe set.
fn cpu_signature() -> Signature {
    Signature::new(HOST, host().into_iter().map(Answer::Bits).collect())
}

#[test]
fn this_device_reports_itself_through_the_api_it_was_opened_through() {
    let device = GPU.report();
    for line in &device.lines {
        println!("device {line}");
    }
    // The line each backend's reader gives that only the API itself could have: a version, or Metal's architecture.
    let readers = [
        ("vulkan", "vulkan driver version: "),
        ("metal", "metal architecture: "),
        ("d3d12", "dxgi driver version: "),
    ];
    let reader = readers.iter().find(|reader| reader.0 == GPU.backend()).unwrap();
    assert!(device.lines.iter().any(|line| line.starts_with(reader.1)));
}

/// What `measure` answers on this machine's GPU, printed as measured through `interface`.
///
/// A second measurement has to answer the same way.
fn measured_twice<Measure, Pending>(interface: &str, measure: Measure) -> Signature
where
    Measure: Fn() -> Pending,
    Pending: Future<Output = Result<Signature, NativeError>>,
{
    let signature = block_on(measure()).unwrap();
    // What the adapter behind the device reports itself as: its name, driver and backend.
    let info = GPU.info();
    let hardware = format!(
        "{} | {} {} | {}",
        info.name, info.driver, info.driver_info, info.backend
    );
    report(interface, &hardware, &signature);
    assert_eq!(signature.fingerprint(), block_on(measure()).unwrap().fingerprint());
    signature
}

#[test]
fn this_gpu_answers_every_clear_the_same_way_twice() {
    let signature = measured_twice("wgpu clear", || GPU.clear_signature());
    // Only rendering to rg11b10ufloat is optional, so only that clear may be refused.
    let refusable = usize::from(!GPU.renders_ufloat11());
    assert_eq!(signature.completed() + refusable, CLEARS.len());
}

#[test]
fn this_gpu_answers_every_fixed_function_probe_the_same_way_twice() {
    let signature = measured_twice("wgpu fixed-function", || GPU.fixed_function_signature());
    assert_eq!(signature.completed(), BLENDS.len() + FILTERS.len() + RASTERS.len());
}

#[test]
fn every_set_is_each_set_beside_the_interface_it_is_filed_under() {
    let sets = block_on(GPU.every_set());
    let interfaces: Vec<&str> = sets.iter().map(|(interface, _)| *interface).collect();
    assert_eq!(
        interfaces,
        [
            "webgpu-split",
            "webgpu-combined",
            "webgpu-clear",
            "webgpu-fixed-function",
            "webgpu-f16-split",
            "webgpu-f16-combined"
        ]
    );
    let split = sets[0].1.as_ref().unwrap();
    assert_eq!(split.fingerprint(), block_on(GPU.signature()).unwrap().fingerprint());
    let fixed = sets[3].1.as_ref().unwrap();
    assert_eq!(fixed.completed(), BLENDS.len() + FILTERS.len() + RASTERS.len());
}

#[test]
fn this_gpu_answers_every_probe_legally_and_the_same_way_twice() {
    let signature = measured_twice("wgpu", || GPU.signature());
    println!("wgpu vendor id: {:#06x}", GPU.info().vendor);
    assert_eq!(signature.completed(), PROBE_COUNT);
    assert!(!signature.traits().contains("=other"));
}

#[test]
fn this_gpu_answers_the_combined_shader_legally_and_the_same_way_twice() {
    let signature = measured_twice("wgpu combined", || GPU.combined_signature());
    assert_eq!(signature.completed(), PROBE_COUNT);
    assert!(!signature.traits().contains("=other"));
}

#[test]
fn this_gpu_answers_the_f16_probes_the_same_way_twice_when_it_has_shader_f16() {
    // A device without shader-f16 compiles none of the set, so it refuses every probe.
    let answered = if GPU.computes_f16() { PROBE_COUNT } else { 0 };
    let split = measured_twice("wgpu f16", || GPU.f16_signature());
    assert_eq!(split.completed(), answered);
    let combined = measured_twice("wgpu f16 combined", || GPU.f16_combined_signature());
    assert_eq!(combined.completed(), answered);
}

// The platforms whose GPU and CPU each have a signature on record: an Apple GPU through Metal, llvmpipe through Vulkan
// and the Basic Render Driver through Direct3D 12. A machine of one of these platforms with hardware not yet on record
// fails here, and the fingerprint the tests beside this one print is what gets it recorded.
#[cfg(any(
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "windows", target_arch = "x86_64"),
))]
#[test]
fn the_registry_has_seen_this_gpu_and_this_cpu_on_this_platform() {
    // An adapter that reports no vendor ID, as Metal's does, is held to its platform alone.
    let vendor = et_gpu_probe::GpuVendor::from_id(GPU.info().vendor);
    let gpu_seen = block_on(GPU.signature()).unwrap().seen().unwrap();
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

#[test]
fn every_adapter_opens_and_answers_the_probe_set() {
    let gpus = block_on(Gpu::open_all());
    assert!(!gpus.is_empty());
    for opened in &gpus {
        let gpu = opened.as_ref().unwrap();
        let info = gpu.info();
        println!(
            "adapter: {} ({:?}) through {}",
            info.name,
            info.device_type,
            gpu.backend()
        );
        // A context that runs no compute shaders, as NVIDIA's EGL gives wgpu, says so for the shader probes, and still
        // answers the clears, which draw.
        match block_on(gpu.signature()) {
            Ok(signature) => assert_eq!(signature.completed(), PROBE_COUNT),
            Err(NativeError::Compute(reason)) => {
                println!("adapter runs no compute shaders: {reason}");
                assert_eq!(block_on(gpu.clear_signature()).unwrap().completed(), CLEARS.len());
            }
            Err(error) => panic!("{} would not answer: {error}", info.name),
        }
        // Vulkan names its driver, and OpenGL its vendor, for the row's `driver`.
        let versions = gpu.report().versions;
        println!("adapter versions: {versions:?}");
        if ["vulkan", "opengl", "opengl-es"].contains(&gpu.backend()) {
            assert!(versions.iter().any(|(field, _)| *field == "driver"), "{versions:?}");
        }
    }
}

#[test]
fn a_context_without_compute_shaders_names_its_backend_and_version() {
    let reason = NoCompute::Context {
        backend: "opengl",
        version: "3.3.0 NVIDIA 580.178.04".to_owned(),
    };
    let error = NativeError::from(reason);
    assert_eq!(
        error.to_string(),
        "the device runs no compute shaders: opengl gives 3.3.0 NVIDIA 580.178.04"
    );
}

#[test]
fn a_device_opened_without_compute_shaders_says_so_and_still_draws() {
    let gpu = block_on(Gpu::open_without_compute(noop_adapter())).unwrap();
    for (interface, outcome) in block_on(gpu.every_set()) {
        let computes = !matches!(interface, "webgpu-clear" | "webgpu-fixed-function");
        match outcome {
            Err(NativeError::Compute(reason)) => {
                assert!(computes, "{interface}");
                let not_asked = NoCompute::NotAsked {
                    adapter: "noop wgpu backend".to_owned(),
                };
                assert_eq!(reason, not_asked);
                assert_eq!(reason.to_string(), "noop wgpu backend was opened without them");
            }
            other => assert!(
                !computes,
                "{interface}: {:?}",
                other.map(|signature| signature.completed())
            ),
        }
    }
}

#[test]
fn the_opengl_backend_is_named_by_the_api_its_context_gives() {
    assert_eq!(
        backend_name(wgpu::Backend::Gl, "4.6 (Core Profile) Mesa 26.1.6"),
        "opengl"
    );
    assert_eq!(
        backend_name(wgpu::Backend::Gl, "OpenGL ES 3.2 Mesa 26.1.6"),
        "opengl-es"
    );
    assert_eq!(backend_name(wgpu::Backend::Vulkan, "OpenGL ES 3.2"), "vulkan");
    assert_eq!(backend_name(wgpu::Backend::Noop, ""), "unknown");
}

#[test]
fn an_opengl_version_string_gives_the_api_and_driver_versions() {
    let owned = |pairs: &[(&'static str, &str)]| -> Vec<(&'static str, String)> {
        pairs
            .iter()
            .map(|(field, value)| (*field, (*value).to_owned()))
            .collect()
    };
    assert_eq!(
        gl_versions("4.6 (Core Profile) Mesa 26.1.6-1pop0"),
        owned(&[
            ("api_version", "OpenGL 4.6"),
            ("driver_version", "(Core Profile) Mesa 26.1.6-1pop0")
        ])
    );
    assert_eq!(
        gl_versions("OpenGL ES 3.2 Mesa 26.1.6"),
        owned(&[("api_version", "OpenGL ES 3.2"), ("driver_version", "Mesa 26.1.6")])
    );
    assert_eq!(
        gl_versions("3.3.0 NVIDIA 580.178.04"),
        owned(&[("api_version", "OpenGL 3.3.0"), ("driver_version", "NVIDIA 580.178.04")])
    );
    assert_eq!(gl_versions("3.3"), owned(&[("api_version", "OpenGL 3.3")]));
    assert_eq!(gl_versions("3.3 "), owned(&[("api_version", "OpenGL 3.3")]));
}

#[test]
fn compute_shaders_run_from_opengl_4_3_and_opengl_es_3_1() {
    let all = wgpu::DownlevelFlags::all();
    let opengl = wgpu::Backend::Gl;
    assert!(runs_compute(opengl, all, "4.6 (Core Profile) Mesa 26.1.6"));
    assert!(runs_compute(opengl, all, "4.3.0 NVIDIA 580.178.04"));
    assert!(!runs_compute(opengl, all, "3.3.0 NVIDIA 580.178.04"));
    assert!(!runs_compute(opengl, all, "4.2 Mesa"));
    assert!(runs_compute(opengl, all, "OpenGL ES 3.1 Mesa 26.1.6"));
    assert!(!runs_compute(opengl, all, "OpenGL ES 3.0 Mesa 26.1.6"));
    // A version this cannot read leaves the flags to decide.
    assert!(runs_compute(opengl, all, "an unknown version"));
    assert!(runs_compute(wgpu::Backend::Vulkan, all, "3.3"));
    let without = wgpu::DownlevelFlags::all() - wgpu::DownlevelFlags::COMPUTE_SHADERS;
    assert!(!runs_compute(wgpu::Backend::Vulkan, without, ""));
}

#[test]
fn an_adapter_no_graphics_api_stands_behind_reports_only_what_wgpu_says() {
    // wgpu's no-op backend is on every platform and has no Vulkan, Metal, Direct3D 12 or OpenGL side, so the report
    // reads none of those APIs: what each reader does for an adapter opened through another API than its own.
    let gpu = block_on(Gpu::open_adapter(noop_adapter())).unwrap();
    assert_eq!(gpu.info().backend, wgpu::Backend::Noop);
    let report = gpu.report();
    assert_ne!(report.lines.len(), 0);
    for api in ["vulkan", "metal", "dxgi", "gl "] {
        assert!(
            !report.lines.iter().any(|line| line.starts_with(api)),
            "{api} in {:?}",
            report.lines
        );
    }
    // wgpu is the whole of the no-op backend, its driver too, and there is no API beneath it to give a version.
    assert_eq!(report.versions, [("driver", "wgpu".to_owned())]);
}

/// The adapter of wgpu's no-op backend, which every platform has.
fn noop_adapter() -> wgpu::Adapter {
    let descriptor = wgpu::InstanceDescriptor {
        backends: wgpu::Backends::NOOP,
        backend_options: wgpu::BackendOptions {
            noop: wgpu::NoopBackendOptions { enable: true },
            ..wgpu::BackendOptions::default()
        },
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    };
    let instance = wgpu::Instance::new(descriptor);
    block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap()
}

/// The adapter wgpu prefers for `power_preference`, opened.
fn preferring(power_preference: wgpu::PowerPreference) -> Gpu {
    let options = wgpu::RequestAdapterOptions {
        power_preference,
        ..wgpu::RequestAdapterOptions::default()
    };
    let gpu = block_on(Gpu::open_with(&options)).unwrap();
    println!("{power_preference:?}: {} through {}", gpu.info().name, gpu.backend());
    gpu
}

#[test]
fn the_adapter_preferred_for_each_power_preference_opens_and_answers_the_probe_set() {
    for power_preference in [wgpu::PowerPreference::HighPerformance, wgpu::PowerPreference::LowPower] {
        let gpu = preferring(power_preference);
        assert_eq!(block_on(gpu.signature()).unwrap().completed(), PROBE_COUNT);
    }
}

/// An Apple silicon Mac has one GPU, so either power preference is given it, and every set answers alike through both.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn both_power_preferences_answer_every_set_the_same_on_apple_silicon() {
    let answers = [wgpu::PowerPreference::HighPerformance, wgpu::PowerPreference::LowPower].map(|power_preference| {
        let gpu = preferring(power_preference);
        let sets = block_on(gpu.every_set()).map(|(interface, set)| (interface, set.unwrap().fingerprint()));
        (gpu.info().name, sets)
    });
    assert_eq!(answers[0], answers[1]);
}

#[cfg(not(target_vendor = "apple"))]
#[test]
fn a_vulkan_driver_version_is_read_in_its_vendors_encoding() {
    use et_gpu_probe::native::{vulkan_api_version, vulkan_driver_version};

    // NVIDIA 580.95.5.0, packed 10.8.8.6 bits.
    assert_eq!(vulkan_driver_version(0x10de, 0x9117_c140, false), "580.95.5.0");
    // Intel's Windows driver 101.130, packed 18.14 bits, but the API layout on Linux, where Mesa drives it.
    assert_eq!(vulkan_driver_version(0x8086, 0x0019_4082, true), "101.130");
    assert_eq!(vulkan_driver_version(0x8086, 0x0680_1006, false), "26.1.6");
    // AMD packs its driver version as the API version is, on every platform.
    assert_eq!(vulkan_driver_version(0x1002, 0x0080_2000, true), "2.2.0");
    assert_eq!(vulkan_api_version(0x0040_6162), "1.6.354");
    assert_eq!(vulkan_api_version(0x2040_6162), "1.6.354 (variant 1)");
}

/// An Intel Mac's GPU gives Metal no architecture, which the report names as none rather than failing over.
///
/// Apple's own GPUs name theirs, which the device report above reads from this machine's device.
#[cfg(target_vendor = "apple")]
#[test]
fn a_metal_device_giving_no_architecture_is_reported_as_none() {
    assert_eq!(et_gpu_probe::native::metal_architecture_name(None), "none");
}
