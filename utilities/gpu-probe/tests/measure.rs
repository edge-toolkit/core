//! Covers measuring adapters and the CPU into a registry: what is added, what a second run leaves alone, and how an
//! adapter that cannot be measured, a registry that cannot be read and an output that cannot be written are reported.
#![cfg(test)]

use std::io::{self, Write};
use std::path::PathBuf;

use et_gpu_probe::native::{Gpu, NativeError};
use et_gpu_probe_server::measure::{MeasureError, measure};
use et_gpu_probe_server::registry::Registry;
use futures_executor::block_on;
use tempfile::TempDir;

/// A registry holding one entry, which no measurement here matches, laid out as `libs/gpu-probe/signatures.yaml` is.
const REGISTRY: &str = r#"# The header, kept as it is.
probe_set: 1
signatures:
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

/// A registry file holding [`REGISTRY`], in a directory removed when the returned guard drops.
fn registry() -> (TempDir, Registry) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("signatures.yaml");
    fs_err::write(&path, REGISTRY).unwrap();
    (dir, Registry::new(path))
}

/// What measuring `opened` into `registry` wrote, beside how it ended.
fn measured(registry: &Registry, opened: Vec<Result<Gpu, NativeError>>) -> (String, Result<(), MeasureError>) {
    let mut out = Vec::new();
    let outcome = measure(registry, opened, &mut out);
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
    assert_eq!(outcome.unwrap_err().to_string(), "1 adapter(s) could not be measured");
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
    let registry = Registry::new(PathBuf::from("no-such-directory/signatures.yaml"));
    let (_out, outcome) = measured(&registry, Vec::new());
    assert!(matches!(outcome, Err(MeasureError::Register(_))), "{outcome:?}");
}

#[test]
fn an_output_that_cannot_be_written_is_reported() {
    let (_dir, registry) = registry();
    let outcome = measure(&registry, vec![unopened()], &mut Broken);
    let error = outcome.unwrap_err();
    assert!(matches!(error, MeasureError::Write(_)), "{error:?}");
    assert_eq!(
        error.to_string(),
        "the output could not be written: the output is closed"
    );
}
