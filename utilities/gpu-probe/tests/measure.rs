//! Covers measuring adapters and the CPU into a registry: what is added, what a second run leaves alone, and how an
//! adapter that cannot be measured, a registry that cannot be read and an output that cannot be written are reported.
#![cfg(test)]

use std::io::{self, Write};

use et_gpu_probe::native::{Gpu, NativeError};
use et_gpu_probe_server::machine::Machine;
use et_gpu_probe_server::measure::{MeasureError, measure, propose_measured};
use et_gpu_probe_server::registry::RegistryTree;
use et_gpu_probe_server::submitted::parse;
use futures_executor::block_on;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

/// A file of entries holding one, which no measurement here matches, laid out as the repository's registry's are.
const ENTRIES: &str = r#"signatures:
  - interface: webgpu
    label: "Tor Browser"
    seen:
      - browser: tor-browser
        os: macos
        cpu_arch: aarch64
        gpu: apple
        backend: unknown
        window: normal
    blocked: true
    reason: "WebGPU is unavailable"
"#;

/// A registry directory holding [`ENTRIES`] as its `webgpu` file, removed when the returned guard drops.
fn registry() -> (TempDir, RegistryTree) {
    let dir = TempDir::new().unwrap();
    fs_err::create_dir(dir.path().join("curated")).unwrap();
    fs_err::write(dir.path().join("curated/webgpu.yaml"), ENTRIES).unwrap();
    let registry = RegistryTree::new(dir.path().to_owned());
    (dir, registry)
}

/// The machine every measurement here is taken on: a runner image, with every field a machine gives.
fn runner() -> Machine {
    Machine::with(&[
        ("cpu_version", "Apple M1 (Virtual)"),
        ("date", "2026-11-23"),
        ("device", "GitHub Actions runner image macos15 20261110.1"),
        ("os_version", "15.7.2"),
        ("source", "https://github.com/edge-toolkit/core/actions/runs/1"),
    ])
}

/// What measuring `opened` into `registry` wrote, beside how it ended.
fn measured(registry: &RegistryTree, opened: Vec<Result<Gpu, NativeError>>) -> (String, Result<(), MeasureError>) {
    let mut out = Vec::new();
    let outcome = measure(registry, &runner(), opened, &mut out);
    (String::from_utf8(out).unwrap(), outcome)
}

/// A wgpu instance with only `backends` in it, the no-op backend switched on.
fn instance(backends: wgpu::Backends) -> wgpu::Instance {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = backends;
    descriptor.backend_options.noop.enable = true;
    wgpu::Instance::new(descriptor)
}

/// An adapter that did not open: an instance with no backend has none to give.
fn unopened() -> Result<Gpu, NativeError> {
    let refused = block_on(instance(wgpu::Backends::empty()).request_adapter(&wgpu::RequestAdapterOptions::default()));
    Err(NativeError::Adapter(refused.unwrap_err()))
}

/// An output every write to fails.
struct Broken;

impl Write for Broken {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("the output is closed"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn other_driver_settings_or_none_are_a_sighting_of_their_own() {
    let (_dir, registry) = registry();
    let fields = [("device", "a desktop"), ("environment", "INTEL_DEBUG=nofc")];
    let outcome = |machine: &Machine| {
        let mut out = Vec::new();
        measure(&registry, machine, Vec::new(), &mut out).unwrap();
        String::from_utf8(out).unwrap()
    };
    let nofc = Machine::with(&fields);
    assert!(outcome(&nofc).contains("added as entry"));
    assert!(outcome(&nofc).contains("already present"));
    // A row with no setting is not covered by the one with `INTEL_DEBUG=nofc`, though it gives less.
    let defaults = Machine::with(&fields[..1]);
    assert!(outcome(&defaults).contains("added a sighting"));
    assert!(outcome(&defaults).contains("already present"));
    let other = Machine::with(&[("device", "a desktop"), ("environment", "RADV_DEBUG=nodcc")]);
    assert!(outcome(&other).contains("added a sighting"));
}

#[test]
fn the_cpu_is_recorded_once_and_left_alone_after() {
    let (_dir, registry) = registry();
    let (first, outcome) = measured(&registry, Vec::new());
    outcome.unwrap();
    assert!(first.contains("host on Rust f32 on the CPU"), "{first}");
    assert!(first.contains("gen:gpu-probe-signatures"), "{first}");
    let (second, outcome) = measured(&registry, Vec::new());
    outcome.unwrap();
    assert!(!second.contains("gen:gpu-probe-signatures"), "{second}");
}

#[test]
fn an_adapter_that_did_not_open_is_passed_over_and_counted() {
    let (_dir, registry) = registry();
    let (out, outcome) = measured(&registry, vec![unopened()]);
    assert!(out.contains("an adapter was not measured"), "{out}");
    assert!(out.contains("host on Rust f32 on the CPU"), "{out}");
    assert_eq!(
        outcome.unwrap_err().to_string(),
        "1 adapter(s) could not be measured in full"
    );
}

#[test]
fn a_device_without_compute_shaders_has_each_shader_set_said_and_counted() {
    let (_dir, registry) = registry();
    let adapter = block_on(instance(wgpu::Backends::NOOP).request_adapter(&wgpu::RequestAdapterOptions::default()));
    let gpu = block_on(Gpu::open_without_compute(adapter.unwrap()));
    let (out, outcome) = measured(&registry, vec![gpu]);
    for set in [
        "webgpu-split",
        "webgpu-combined",
        "webgpu-f16-split",
        "webgpu-f16-combined",
    ] {
        let said = format!("{set} on noop wgpu backend was not measured: the device runs no compute shaders: ");
        assert!(out.contains(&said), "{set}: {out}");
    }
    // The clears and the fixed-function probes draw rather than compute, so they are still recorded.
    for set in ["webgpu-clear", "webgpu-fixed-function"] {
        assert!(
            out.contains(&format!("{set} on noop wgpu backend: added")),
            "{set}: {out}"
        );
    }
    assert!(out.contains("host on Rust f32 on the CPU"), "{out}");
    assert!(matches!(outcome, Err(MeasureError::Skipped(1))), "{outcome:?}");
}

#[test]
fn the_noop_adapter_is_measured_or_passed_over_and_the_cpu_still_recorded() {
    let (_dir, registry) = registry();
    let adapter = block_on(instance(wgpu::Backends::NOOP).request_adapter(&wgpu::RequestAdapterOptions::default()));
    let gpu = block_on(Gpu::open_adapter(adapter.unwrap()));
    let (out, outcome) = measured(&registry, vec![gpu]);
    assert!(out.contains("host on Rust f32 on the CPU"), "{out}");
    match outcome {
        Ok(()) => assert!(out.contains("webgpu-split on "), "{out}"),
        Err(skipped) => assert!(matches!(skipped, MeasureError::Skipped(1)), "{skipped}"),
    }
}

#[test]
fn a_registry_that_cannot_be_read_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing");
    let registry = RegistryTree::new(missing.clone());
    let (_out, outcome) = measured(&registry, Vec::new());
    assert!(matches!(outcome, Err(MeasureError::Register(_))), "{outcome:?}");
    assert!(!missing.exists());
}

#[test]
fn an_output_that_cannot_be_written_is_reported() {
    let (_dir, registry) = registry();
    let outcome = measure(&registry, &runner(), vec![unopened()], &mut Broken);
    let error = outcome.unwrap_err();
    assert!(matches!(error, MeasureError::Write(_)), "{error:?}");
    assert_eq!(
        error.to_string(),
        "the output could not be written: the output is closed"
    );
}

#[test]
fn the_cpu_row_carries_the_machine_and_the_runtime_it_was_built_with() {
    let dir = tempfile::tempdir().unwrap();
    let mut out = Vec::new();
    let path = propose_measured(dir.path(), &runner(), Vec::new(), &mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("1 signature(s) proposed in "));
    assert!(path.starts_with(dir.path()));
    let submissions = parse(&fs_err::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(submissions.len(), 1);
    let cpu = &submissions[0];
    assert_eq!(cpu.interface, "host");
    for field in ["cpu_version", "date", "device", "os_version", "source"] {
        assert_eq!(cpu.seen.get(field), runner().fields().get(field));
    }
    let runtime = &cpu.seen["runtime"];
    assert!(runtime.starts_with("Rust f32, built for "));
    assert!(runtime.contains(std::env::consts::ARCH));
    assert!(cpu.label.starts_with("Rust f32 on the CPU: Apple M1 (Virtual), "));
    let machine = format!(
        " {} 15.7.2 {}, GitHub Actions runner image macos15 20261110.1",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    assert!(cpu.label.ends_with(&machine));
}

#[test]
fn an_adapter_passed_over_still_leaves_the_file_written_and_counted() {
    let dir = tempfile::tempdir().unwrap();
    let mut out = Vec::new();
    let outcome = propose_measured(dir.path(), &runner(), vec![unopened()], &mut out);
    assert!(matches!(outcome, Err(MeasureError::Skipped(1))));
    assert_eq!(fs_err::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn a_directory_that_cannot_be_written_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join("a file");
    fs_err::write(&blocked, "").unwrap();
    let outcome = propose_measured(&blocked, &runner(), Vec::new(), &mut Vec::new());
    assert!(matches!(outcome, Err(MeasureError::Propose(_))));
}
