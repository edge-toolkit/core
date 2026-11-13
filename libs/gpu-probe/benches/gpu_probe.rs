//! Benchmarks every operation `et-gpu-probe` offers: building the probe sets and their reference answers, the texture
//! formats, reading a signature, the registry, and putting each probe set to every GPU adapter the machine has.
//!
//! Run with `cargo bench -p et-gpu-probe --features native,registry`. The GPU group names each adapter as wgpu does,
//! so its results are this machine's own; the rest measure the CPU alone.
#![expect(
    clippy::single_call_fn,
    reason = "criterion calls each benchmark group once, through criterion_group!, and each helper serves one group"
)]

use std::hint::black_box;

use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, Criterion, criterion_group, criterion_main};
use et_gpu_probe::native::Gpu;
use et_gpu_probe::{
    Answer, BLENDS, FILTERS, Format, GpuVendor, HOST, RASTER_FORMAT, RASTER_WIDTH, RASTERS, Sighting, Signature,
    WEBGL2, WEBGPU, WEBGPU_CLEAR, WEBGPU_FIXED_FUNCTION, clear_reference, combined_shader, coverage, every_sighting,
    fixed_function_reference, fragment_shaders, host, reference, shaders,
};
use futures_executor::block_on;
use serde_json::json;

/// Every texture format the probe sets use.
const FORMATS: [Format; 5] = [
    Format::Rgba8Unorm,
    Format::Rgba8UnormSrgb,
    Format::Rgb10a2Unorm,
    Format::Rg11b10Ufloat,
    Format::Rgba16Float,
];

/// Every interface a signature of the probe sets can be read through, beside the answers a device would give.
const INTERFACES: [&str; 5] = [HOST, WEBGPU, WEBGL2, WEBGPU_CLEAR, WEBGPU_FIXED_FUNCTION];

/// Measure `routine` as `id` in `group`; criterion keeps what it returns from being optimised away.
///
/// An `id` of the form `function/parameter` is reported as criterion reports a parameterised benchmark.
fn measure<Output, Routine>(group: &mut BenchmarkGroup<'_, WallTime>, id: &str, mut routine: Routine)
where
    Routine: FnMut() -> Output,
{
    let _chained: &mut BenchmarkGroup<'_, WallTime> = group.bench_function(id, |bench| bench.iter(&mut routine));
}

/// Building the shader probe set, the GPU-free answers to it, and the shaders of the sets no compiler decides.
fn probe_sets(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("probe_sets");
    measure(&mut group, "shaders", shaders);
    measure(&mut group, "combined_shader", combined_shader);
    measure(&mut group, "fragment_shaders", fragment_shaders);
    measure(&mut group, "reference", reference);
    measure(&mut group, "host", host);
    measure(&mut group, "clear_reference", clear_reference);
    measure(&mut group, "fixed_function_reference", fixed_function_reference);
    for blend in BLENDS {
        measure(&mut group, &format!("wgsl/{}", blend.name), || blend.shader());
        measure(&mut group, &format!("glsl/{}", blend.name), || blend.glsl());
        measure(&mut group, &format!("reference/{}", blend.name), || blend.reference());
    }
    for filter in FILTERS {
        measure(&mut group, &format!("wgsl/{}", filter.name), || filter.shader());
        measure(&mut group, &format!("glsl/{}", filter.name), || filter.glsl());
        measure(&mut group, &format!("reference/{}", filter.name), || filter.reference());
    }
    for raster in RASTERS {
        measure(&mut group, &format!("wgsl/{}", raster.name), || raster.shader());
        measure(&mut group, &format!("glsl/{}", raster.name), || raster.glsl());
        measure(&mut group, &format!("reference/{}", raster.name), || raster.reference());
    }
    let target_bytes = RASTER_WIDTH.saturating_mul(RASTER_FORMAT.texel_bytes());
    let texels = vec![0xff_u8; usize::try_from(target_bytes).unwrap_or(0)];
    measure(&mut group, "coverage", || coverage(black_box(&texels)));
    group.finish();
}

/// Converting a value to and from each format's code, and reading a code out of a texel.
fn formats(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("formats");
    for format in FORMATS {
        let name = format.name();
        let code = format.encode(0.5_f64);
        let first_word: [u8; 4] = format.texel(code).first_chunk().copied().unwrap_or_default();
        measure(&mut group, &format!("encode/{name}"), || {
            format.encode(black_box(0.3_f64))
        });
        measure(&mut group, &format!("decode/{name}"), || format.decode(black_box(code)));
        measure(&mut group, &format!("texel/{name}"), || format.texel(black_box(code)));
        measure(&mut group, &format!("red/{name}"), || format.red(black_box(first_word)));
    }
    group.finish();
}

/// A signature through `interface` answering each probe as a device computing it exactly would.
fn answered(interface: &'static str) -> Signature {
    let answers = match interface {
        HOST => host().to_vec(),
        WEBGPU_CLEAR => clear_reference(),
        WEBGPU_FIXED_FUNCTION => fixed_function_reference(),
        _ => reference().to_vec(),
    };
    Signature::new(interface, answers.into_iter().map(Answer::Bits).collect())
}

/// Reading a signature through each interface, and finding it in the registry.
///
/// That is its words, fingerprint, traits and description, then the registry entries and rows that hold it.
fn signatures(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("signatures");
    for interface in INTERFACES {
        let signature = answered(interface);
        measure(&mut group, &format!("results/{interface}"), || signature.results());
        measure(&mut group, &format!("fingerprint/{interface}"), || {
            signature.fingerprint()
        });
        measure(&mut group, &format!("completed/{interface}"), || signature.completed());
        measure(&mut group, &format!("deviations/{interface}"), || {
            signature.deviations()
        });
        measure(&mut group, &format!("traits/{interface}"), || signature.traits());
        measure(&mut group, &format!("describe/{interface}"), || signature.describe());
        measure(&mut group, &format!("class/{interface}"), || signature.class());
        measure(&mut group, &format!("identify/{interface}"), || signature.identify());
        measure(&mut group, &format!("seen/{interface}"), || signature.seen());
    }
    group.finish();
}

/// Reading the registry's rows, and naming GPU vendors.
///
/// The row read is laid out as the registry's own are, with the free-text fields a row carries beside the fixed ones,
/// and the entry holds three of it, as an entry seen in a few places does.
fn registry(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("registry");
    let row = json!({
        "browser": "chrome",
        "browser_version": "157.0.8103.78",
        "os": "linux",
        "cpu_arch": "x86_64",
        "gpu": "nvidia",
        "gpu_version": "NVIDIA RTX A1000 Laptop GPU",
        "backend": "vulkan",
        "device": "Dell Precision 5570",
        "window": "normal",
    });
    let entry = json!({ "interface": WEBGPU, "label": "A GPU", "seen": [row, row, row] });
    measure(&mut group, "every_sighting", every_sighting);
    measure(&mut group, "sighting_read", || Sighting::read(black_box(&row)));
    measure(&mut group, "sighting_read_entry", || {
        Sighting::read_entry(black_box(&entry))
    });
    if let Ok(sighting) = Sighting::read(&row) {
        measure(&mut group, "sighting_is_here", || sighting.is_here());
    }
    measure(&mut group, "gpu_vendor_from_id", || {
        GpuVendor::from_id(black_box(0x10de))
    });
    measure(&mut group, "gpu_vendor_from_device_name", || {
        GpuVendor::from_device_name(black_box("Apple M5 Pro"))
    });
    measure(&mut group, "gpu_vendor_name", || black_box(GpuVendor::Nvidia).name());
    group.finish();
}

/// Opening the GPUs, and putting every probe set to each adapter this machine has.
///
/// A GPU answers in milliseconds where the CPU groups take nanoseconds, so this group takes fewer samples. Each adapter
/// is named as wgpu names it, beside the API it was opened through; one whose device does not open is left out, as
/// `Gpu::open_all` reports it.
fn gpus(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("gpus");
    let _sampled: &mut BenchmarkGroup<'_, WallTime> = group.sample_size(10);
    measure(&mut group, "open", || block_on(Gpu::open()));
    measure(&mut group, "open_all", || block_on(Gpu::open_all()));
    for gpu in block_on(Gpu::open_all()).into_iter().flatten() {
        let name = format!("{} ({})", gpu.info().name, gpu.backend());
        measure(&mut group, &format!("signature/{name}"), || block_on(gpu.signature()));
        measure(&mut group, &format!("combined_signature/{name}"), || {
            block_on(gpu.combined_signature())
        });
        measure(&mut group, &format!("clear_signature/{name}"), || {
            block_on(gpu.clear_signature())
        });
        measure(&mut group, &format!("fixed_function_signature/{name}"), || {
            block_on(gpu.fixed_function_signature())
        });
        measure(&mut group, &format!("report/{name}"), || gpu.report());
    }
    group.finish();
}

criterion_group!(benches, probe_sets, formats, signatures, registry, gpus);
criterion_main!(benches);
