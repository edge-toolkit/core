//! Puts the probe sets to this machine with no browser involved, to its GPU through wgpu and to its CPU.
//!
//! Each shader signature is checked against the one recorded for that hardware. A machine nobody has recorded still has
//! to answer every probe legally, and the same way twice; what it answered is printed, which is how its signature comes
//! to be recorded. The clears and fixed-function probes, which no shader compiler decides, have to be answered the same
//! way twice. Beside the signatures the test prints what the device says it is, read from the API it was opened through
//! rather than from what wgpu makes of it.
#![cfg(test)]

use std::sync::LazyLock;

use et_gpu_probe::native::{Gpu, NativeError};
use et_gpu_probe::{Answer, BLENDS, CLEARS, FILTERS, HOST, PROBE_COUNT, RASTERS, Signature, host};
use futures_executor::block_on;

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
        assert_eq!(block_on(gpu.signature()).unwrap().completed(), PROBE_COUNT);
    }
}

#[test]
fn an_adapter_no_graphics_api_stands_behind_reports_only_what_wgpu_says() {
    // wgpu's no-op backend is on every platform and has no Vulkan, Metal or Direct3D 12 side, so the report reads
    // none of those APIs: what each reader does for an adapter opened through another API than its own.
    let descriptor = wgpu::InstanceDescriptor {
        backends: wgpu::Backends::NOOP,
        backend_options: wgpu::BackendOptions {
            noop: wgpu::NoopBackendOptions { enable: true },
            ..wgpu::BackendOptions::default()
        },
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    };
    let instance = wgpu::Instance::new(descriptor);
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
    let gpu = block_on(Gpu::open_adapter(adapter)).unwrap();
    assert_eq!(gpu.info().backend, wgpu::Backend::Noop);
    let report = gpu.report();
    assert_ne!(report.lines.len(), 0);
    for api in ["vulkan", "metal", "dxgi"] {
        assert!(
            !report.lines.iter().any(|line| line.starts_with(api)),
            "{api} in {:?}",
            report.lines
        );
    }
    assert_eq!(report.versions.len(), 0);
}

#[test]
fn the_adapter_preferred_for_each_power_preference_opens_and_answers_the_probe_set() {
    for power_preference in [wgpu::PowerPreference::HighPerformance, wgpu::PowerPreference::LowPower] {
        let options = wgpu::RequestAdapterOptions {
            power_preference,
            ..wgpu::RequestAdapterOptions::default()
        };
        let gpu = block_on(Gpu::open_with(&options)).unwrap();
        println!("{power_preference:?}: {} through {}", gpu.info().name, gpu.backend());
        assert_eq!(block_on(gpu.signature()).unwrap().completed(), PROBE_COUNT);
    }
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
