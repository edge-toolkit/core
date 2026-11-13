//! Put the probe sets to GPU adapters and to this machine's CPU, and add to a registry each sighting it does not yet
//! hold.
//!
//! Each signature is filed under the interface the registry gives a probe set run with no browser, and seen at a row
//! built from what the device says it is: its vendor, name and backend from wgpu, and its driver and API versions from
//! the API beneath. A row the registry already records, with every field the same, is left alone, so measuring twice
//! adds nothing the second time.
#![expect(
    clippy::single_call_fn,
    reason = "each step of a measurement is named, and called once"
)]

use std::collections::BTreeMap;
use std::io::Write;

use et_gpu_probe::native::{Gpu, NativeError};
use et_gpu_probe::{Answer, GpuVendor, HOST, Signature, host};
use futures_executor::block_on;

use crate::registry::{Outcome, RegisterError, Registry, Submission};

/// The operating system and CPU architecture this runs on, as a registry row names them.
const HERE: (&str, &str) = (std::env::consts::OS, std::env::consts::ARCH);

/// A signature measured through one interface, beside the label it is filed under if new and the row it was seen at.
type Measured = (&'static str, Signature, String, BTreeMap<String, String>);

/// What stopped a measurement being recorded in full.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MeasureError {
    /// The registry refused a signature.
    #[error(transparent)]
    Register(#[from] RegisterError),
    /// What became of a signature could not be written out.
    #[error("the output could not be written: {0}")]
    Write(#[from] std::io::Error),
    /// Some adapters could not be measured, by how many; every other one was recorded all the same.
    #[error("{0} adapter(s) could not be measured")]
    Skipped(usize),
}

/// Measure every adapter in `opened` and this machine's CPU, and add what they answer to `registry`.
///
/// What became of each signature is written to `out`. An adapter that did not open, or would not answer, is said so
/// and passed over: the others are measured all the same, and the count of those passed over is the error returned
/// once every other one is recorded.
pub fn measure<Out>(
    registry: &Registry,
    opened: Vec<Result<Gpu, NativeError>>,
    out: &mut Out,
) -> Result<(), MeasureError>
where
    Out: Write,
{
    let mut measured = Vec::new();
    let mut skipped = 0_usize;
    for gpu in opened {
        match gpu.and_then(|gpu| gpu_signatures(&gpu)) {
            Ok(signatures) => measured.extend(signatures),
            Err(error) => {
                writeln!(out, "an adapter was not measured: {error}")?;
                skipped = skipped.saturating_add(1);
            }
        }
    }
    measured.push(cpu_signature());
    let mut added = false;
    for (interface, signature, label, seen) in measured {
        let outcome = record(registry, interface, &signature, &label, seen)?;
        writeln!(out, "{interface} on {}: {outcome}", signature_label(&label))?;
        added |= !matches!(outcome, Outcome::Present(_));
    }
    if added {
        writeln!(
            out,
            "run `mise run gen:gpu-probe-signatures` to refresh the JSON the et-gpu-probe crate embeds"
        )?;
    }
    if skipped > 0 {
        return Err(MeasureError::Skipped(skipped));
    }
    Ok(())
}

/// Every probe set `gpu` answers, each beside the label and row a sighting of it is recorded with.
fn gpu_signatures(gpu: &Gpu) -> Result<[Measured; 4], NativeError> {
    let info = gpu.info();
    let (system, arch) = HERE;
    let mut seen = row(&[
        ("browser", "none"),
        ("os", system),
        ("cpu_arch", arch),
        ("gpu", vendor_of(info.vendor, &info.name).name()),
        ("gpu_version", &info.name),
        ("backend", gpu.backend()),
        ("window", "none"),
    ]);
    for (field, value) in gpu.report().versions {
        let _previous: Option<String> = seen.insert(field.to_owned(), value);
    }
    let label = format!(
        "{} through {} on {system} {arch} -- wgpu, no browser",
        info.name,
        gpu.backend()
    );
    Ok([
        ("webgpu-split", block_on(gpu.signature())?, label.clone(), seen.clone()),
        (
            "webgpu-combined",
            block_on(gpu.combined_signature())?,
            label.clone(),
            seen.clone(),
        ),
        (
            "webgpu-clear",
            block_on(gpu.clear_signature())?,
            label.clone(),
            seen.clone(),
        ),
        (
            "webgpu-fixed-function",
            block_on(gpu.fixed_function_signature())?,
            label,
            seen,
        ),
    ])
}

/// Whose GPU an adapter is: by the vendor `id` it reports, or by its `name` where the backend, like Metal, gives none.
fn vendor_of(id: u32, name: &str) -> GpuVendor {
    let by_id = GpuVendor::from_id(id);
    if by_id == GpuVendor::Unknown {
        GpuVendor::from_device_name(name)
    } else {
        by_id
    }
}

/// What this machine's CPU answers, beside the label and row a sighting of it is recorded with.
fn cpu_signature() -> Measured {
    let (system, arch) = HERE;
    let seen = row(&[
        ("browser", "none"),
        ("os", system),
        ("cpu_arch", arch),
        ("gpu", "none"),
        ("backend", "none"),
        ("window", "none"),
    ]);
    let signature = Signature::new(HOST, host().into_iter().map(Answer::Bits).collect());
    (HOST, signature, format!("Rust f32 on the CPU: {system} {arch}"), seen)
}

/// Add `signature`, measured through `interface` and seen at `seen`, to `registry`.
fn record(
    registry: &Registry,
    interface: &str,
    signature: &Signature,
    label: &str,
    seen: BTreeMap<String, String>,
) -> Result<Outcome, RegisterError> {
    let results = signature
        .results()
        .into_iter()
        .map(|(name, word)| (name.to_owned(), word))
        .collect();
    registry.register(&Submission::signature(interface, label, results, seen))
}

/// The GPU or CPU part of `label`, which is what tells one device's lines of output from another's.
fn signature_label(label: &str) -> &str {
    label.split(" through ").next().unwrap_or(label)
}

/// A registry row from its `fields`.
fn row(fields: &[(&str, &str)]) -> BTreeMap<String, String> {
    fields
        .iter()
        .map(|(field, value)| ((*field).to_owned(), (*value).to_owned()))
        .collect()
}
