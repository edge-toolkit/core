//! Put the probe sets to GPU adapters and to this machine's CPU, and record each sighting: in a registry, which adds
//! each it does not yet hold, or as a file of submissions for a registry elsewhere.
//!
//! Each signature is filed under the interface the registry gives a probe set run with no browser, and seen at a row
//! built from what the device says it is -- its vendor, name and backend from wgpu, and its driver and API versions
//! from the API beneath -- from what the [`Machine`] says of the machine it ran on, and from the runtime that measured
//! it.
//! A row the registry already records, with every field the same, is left alone, so measuring twice adds nothing the
//! second time.
#![expect(
    clippy::single_call_fn,
    reason = "each step of a measurement is named, and called once"
)]

use std::io::Write;
use std::path::{Path, PathBuf};

use et_gpu_probe::native::{Gpu, NativeError};
use et_gpu_probe::{Answer, GpuVendor, HOST, Signature, host};
use futures_executor::block_on;

use crate::built_info;
use crate::machine::Machine;
use crate::registry::{Outcome, RegisterError, RegistryTree, Submission, row};
use crate::submitted::{ProposeError, propose};

/// The operating system and CPU architecture this runs on, as a registry row names them.
const HERE: (&str, &str) = (std::env::consts::OS, std::env::consts::ARCH);

/// What stopped a measurement being recorded in full.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MeasureError {
    /// The registry refused a signature.
    #[error(transparent)]
    Register(#[from] RegisterError),
    /// The file of submissions could not be written.
    #[error(transparent)]
    Propose(#[from] ProposeError),
    /// What became of a signature could not be written out.
    #[error("the output could not be written: {0}")]
    Write(#[from] std::io::Error),
    /// Some adapters could not be measured, by how many; every other one was recorded all the same.
    #[error("{0} adapter(s) could not be measured")]
    Skipped(usize),
}

/// Measure every adapter in `opened` and this machine's CPU, `machine`, and add what they answer to `registry`.
///
/// What became of each signature is written to `out`. An adapter that did not open, or would not answer, is said so
/// and passed over: the others are measured all the same, and the count of those passed over is the error returned once
/// every other one is recorded.
pub fn measure<Out>(
    registry: &RegistryTree,
    machine: &Machine,
    opened: Vec<Result<Gpu, NativeError>>,
    out: &mut Out,
) -> Result<(), MeasureError>
where
    Out: Write,
{
    let (submissions, skipped) = measured(machine, opened, out)?;
    let mut added = false;
    for submission in &submissions {
        let outcome = registry.register(submission)?;
        writeln!(
            out,
            "{} on {}: {outcome}",
            submission.interface,
            signature_label(&submission.label)
        )?;
        added |= !matches!(outcome, Outcome::Present(_));
    }
    if added {
        writeln!(
            out,
            "run `mise run gen:gpu-probe-signatures` to refresh the JSON the et-gpu-probe crate embeds"
        )?;
    }
    passed_over(skipped)
}

/// Measure as [`measure`] does, but write what is measured as a file of submissions in the directory `dir`.
///
/// That is for a registry this cannot write to, such as on a CI runner. The file's path is written to `out`. An
/// adapter passed over is said so, and counted in the error, as there.
pub fn propose_measured<Out>(
    dir: &Path,
    machine: &Machine,
    opened: Vec<Result<Gpu, NativeError>>,
    out: &mut Out,
) -> Result<PathBuf, MeasureError>
where
    Out: Write,
{
    let (submissions, skipped) = measured(machine, opened, out)?;
    let path = propose(dir, &submissions)?;
    writeln!(out, "{} signature(s) proposed in {}", submissions.len(), path.display())?;
    passed_over(skipped)?;
    Ok(path)
}

/// Every signature the adapters in `opened` and this machine's CPU answer, beside how many adapters were passed over.
fn measured<Out>(
    machine: &Machine,
    opened: Vec<Result<Gpu, NativeError>>,
    out: &mut Out,
) -> Result<(Vec<Submission>, usize), MeasureError>
where
    Out: Write,
{
    let mut submissions = Vec::new();
    let mut skipped = 0_usize;
    for gpu in opened {
        match gpu.and_then(|gpu| gpu_signatures(&gpu, machine)) {
            Ok(signatures) => submissions.extend(signatures),
            Err(error) => {
                writeln!(out, "an adapter was not measured: {error}")?;
                skipped = skipped.saturating_add(1);
            }
        }
    }
    submissions.push(cpu_signature(machine));
    Ok((submissions, skipped))
}

/// Nothing wrong where `skipped` is none, and otherwise the error counting the adapters passed over.
const fn passed_over(skipped: usize) -> Result<(), MeasureError> {
    if skipped > 0 {
        return Err(MeasureError::Skipped(skipped));
    }
    Ok(())
}

/// The version of `package` this crate was built with, as `Cargo.lock` recorded it; empty if it was not one.
fn built_with(package: &str) -> &'static str {
    built_info::DEPENDENCIES
        .iter()
        .find(|(name, _)| *name == package)
        .map_or("", |(_, version)| version)
}

/// Every probe set `gpu` answers, each as a submission at the row `machine` and the adapter make.
fn gpu_signatures(gpu: &Gpu, machine: &Machine) -> Result<Vec<Submission>, NativeError> {
    let info = gpu.info();
    let (system, arch) = HERE;
    let runtime = format!("wgpu {}, built for {}", built_with("wgpu"), built_info::TARGET);
    let mut seen = row(&[
        ("browser", "none"),
        ("os", system),
        ("cpu_arch", arch),
        ("gpu", vendor_of(info.vendor, &info.name).name()),
        ("gpu_version", &info.name),
        ("backend", gpu.backend()),
        ("runtime", &runtime),
        ("window", "none"),
    ]);
    for (field, value) in gpu.report().versions {
        let _previous: Option<String> = seen.insert(field.to_owned(), value);
    }
    let seen = machine.row(seen);
    let label = format!(
        "{} through {} on {} -- wgpu, no browser",
        info.name,
        gpu.backend(),
        machine.named(system, arch)
    );
    block_on(gpu.every_set())
        .into_iter()
        .map(|(interface, outcome)| Ok(Submission::measured(interface, &outcome?, &label, seen.clone())))
        .collect()
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

/// What this machine's CPU answers, as a submission at the row `machine` makes.
fn cpu_signature(machine: &Machine) -> Submission {
    let (system, arch) = HERE;
    let runtime = format!("Rust f32, built for {}", built_info::TARGET);
    let seen = machine.row(row(&[
        ("browser", "none"),
        ("os", system),
        ("cpu_arch", arch),
        ("gpu", "none"),
        ("backend", "none"),
        ("runtime", &runtime),
        ("window", "none"),
    ]));
    let signature = Signature::new(HOST, host().into_iter().map(Answer::Bits).collect());
    let cpu = machine.fields().get("cpu_version").map_or(arch, String::as_str);
    let label = format!("Rust f32 on the CPU: {cpu}, {}", machine.named(system, arch));
    Submission::measured(HOST, &signature, &label, seen)
}

/// The GPU or CPU part of `label`, which is what tells one device's lines of output from another's.
fn signature_label(label: &str) -> &str {
    label.split(" through ").next().unwrap_or(label)
}
