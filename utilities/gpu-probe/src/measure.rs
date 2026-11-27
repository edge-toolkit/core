//! Put the probe sets to GPU adapters and to this machine's CPU, and record each sighting: in a registry, which adds
//! each it does not yet hold, or as a file of submissions for a registry elsewhere.
//!
//! Each signature is filed under the interface the registry gives a probe set run with no browser, and seen at a row
//! built from what the device says it is -- its vendor, name and backend from wgpu, and its driver and API versions
//! from the API beneath -- from what the [`Machine`] says of the machine it ran on, and from the runtime that measured
//! it. A row the registry already records, with every field the same, is left alone, so measuring twice adds nothing
//! the second time.
//!
//! Each adapter's memory is recorded beside its signatures, as JSON the registry has no place for yet: what the API
//! beneath wgpu says of it, and how far a walk capped at [`MEMORY_CAP`] went.
#![expect(
    clippy::single_call_fn,
    reason = "each step of a measurement is named, and called once"
)]

use std::io::Write;
use std::path::{Path, PathBuf};

use et_gpu_probe::native::{Gpu, NativeError};
use et_gpu_probe::{Answer, GpuVendor, HOST, Signature, host};
use futures_executor::block_on;
use serde_json::{Map, Value};

use crate::built_info;
use crate::machine::Machine;
use crate::registry::{Outcome, RegisterError, RegistryTree, Submission, row};
use crate::submitted::{ProposeError, name_of, propose};

/// The operating system and CPU architecture this runs on, as a registry row names them.
const HERE: (&str, &str) = (std::env::consts::OS, std::env::consts::ARCH);

/// The most a memory walk allocates on one adapter: 1 GiB.
///
/// A hosted runner's adapter is a software renderer or a virtual GPU whose memory is the runner's own, of 7 GiB and up,
/// so an uncapped walk could exhaust the machine it measures.
pub const MEMORY_CAP: u64 = 1 << 30_u32;

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
    /// Some adapters could not be measured in full, by how many; every set that answered was recorded all the same.
    #[error("{0} adapter(s) could not be measured in full")]
    Skipped(usize),
}

/// Measure every adapter in `opened` and this machine's CPU, `machine`, and add what they answer to `registry`.
///
/// What became of each signature is written to `out`. An adapter that did not open is said so and passed over, as
/// is each set an adapter would not answer: everything else is measured all the same, and the count of adapters not
/// measured in full is the error returned once every other signature is recorded.
pub fn measure<Out>(
    registry: &RegistryTree,
    machine: &Machine,
    opened: Vec<Result<Gpu, NativeError>>,
    out: &mut Out,
) -> Result<(), MeasureError>
where
    Out: Write,
{
    let (submissions, skipped, _memory) = measured(machine, opened, out)?;
    let mut added = false;
    for submission in &submissions {
        let outcome = registry.register(submission)?;
        writeln!(
            out,
            "{} on {}: {outcome}",
            submission.interface,
            signature_label(&submission.label),
        )?;
        added |= !matches!(outcome, Outcome::Present(_));
    }
    if added {
        writeln!(
            out,
            "run `mise run gen:gpu-probe-signatures` to refresh the JSON the et-gpu-probe crate embeds",
        )?;
    }
    passed_over(skipped)
}

/// Measure as [`measure`] does, but write what is measured as a file of submissions in the directory `dir`.
///
/// That is for a registry this cannot write to, such as on a CI runner. The file's path is written to `out`. An adapter
/// passed over is said so, and counted in the error, as there.
///
/// Each adapter's memory is written beside it as a JSON array in `memory-<name>.json`, which `gpu-probe-fold` leaves
/// alone, as it files only `.txt` files; with no adapter measured, there is none.
pub fn propose_measured<Out>(
    dir: &Path,
    machine: &Machine,
    opened: Vec<Result<Gpu, NativeError>>,
    out: &mut Out,
) -> Result<PathBuf, MeasureError>
where
    Out: Write,
{
    let (submissions, skipped, memory) = measured(machine, opened, out)?;
    let path = propose(dir, &submissions)?;
    writeln!(out, "{} signature(s) proposed in {}", submissions.len(), path.display())?;
    if !memory.is_empty() {
        let count = memory.len();
        let text = format!("{:#}\n", Value::Array(memory));
        let memory_path = dir.join(Path::new(&format!("memory-{}", name_of(&text))).with_extension("json"));
        fs_err::write(&memory_path, text)?;
        writeln!(out, "{count} adapter(s)' memory written to {}", memory_path.display())?;
    }
    passed_over(skipped)?;
    Ok(path)
}

/// Every signature the adapters in `opened` and this machine's CPU answer, and what each adapter says of its memory.
///
/// Beside them is how many adapters were passed over.
fn measured<Out>(
    machine: &Machine,
    opened: Vec<Result<Gpu, NativeError>>,
    out: &mut Out,
) -> Result<(Vec<Submission>, usize, Vec<Value>), MeasureError>
where
    Out: Write,
{
    let mut submissions = Vec::new();
    let mut memory = Vec::new();
    let mut skipped = 0_usize;
    for gpu in opened {
        let gpu = match gpu {
            Ok(gpu) => gpu,
            Err(error) => {
                writeln!(out, "an adapter was not measured: {error}")?;
                skipped = skipped.saturating_add(1);
                continue;
            }
        };
        let mut in_full = true;
        for (interface, outcome) in gpu_signatures(&gpu, machine) {
            match outcome {
                Ok(submission) => submissions.push(submission),
                Err(error) => {
                    writeln!(out, "{interface} on {} was not measured: {error}", gpu.info().name)?;
                    in_full = false;
                }
            }
        }
        if !in_full {
            skipped = skipped.saturating_add(1);
        }
        let record = memory_of(&gpu, &adapter_label(&gpu, machine));
        writeln!(out, "memory: {record}")?;
        memory.push(record);
    }
    submissions.push(cpu_signature(machine));
    Ok((submissions, skipped, memory))
}

/// What `gpu`, named `label`, says of its memory, and how far a walk capped at [`MEMORY_CAP`] went.
///
/// The record holds the `label`, then either the `api` the figure came from with its `local`, `budget` and `unified`
/// fields, as `ApiMemory` gives them, or the `api_error` saying why there is none, then the walk's `cap` and either
/// the `walked` bytes and whether it was `refused`, its `walk_error`, or why it was not run, as `walk_skipped`. A CPU
/// adapter's memory is the machine's own and the no-op backend's is none at all, so neither is walked.
fn memory_of(gpu: &Gpu, label: &str) -> Value {
    let mut record = Map::new();
    let mut put = |key: &str, value: Value| {
        let _previous: Option<Value> = record.insert(key.to_owned(), value);
    };
    put("label", label.into());
    match gpu.api_memory() {
        Ok(api) => {
            put("api", api.api.into());
            put("local", api.local.into());
            put("budget", api.budget.into());
            put("unified", api.unified.into());
        }
        Err(error) => put("api_error", error.to_string().into()),
    }
    put("cap", MEMORY_CAP.into());
    let info = gpu.info();
    // The no-op adapter says it is a CPU, so it is told apart first.
    if info.backend == wgpu::Backend::Noop {
        put("walk_skipped", "the no-op backend holds no memory".into());
    } else if info.device_type == wgpu::DeviceType::Cpu {
        put("walk_skipped", "a CPU adapter's memory is the machine's own".into());
    } else {
        match block_on(gpu.walk_memory(MEMORY_CAP)) {
            Ok(walk) => {
                put("walked", walk.bytes.into());
                put("refused", walk.refused.into());
            }
            Err(error) => put("walk_error", error.to_string().into()),
        }
    }
    Value::Object(record)
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

/// Every probe set put to `gpu`, beside the submission at the row `machine` and the adapter make, or why it gave none.
///
/// Each set is its own outcome, so a device that answers the clears but runs no compute shaders, say, still gives the
/// sets it answered.
fn gpu_signatures(gpu: &Gpu, machine: &Machine) -> Vec<(&'static str, Result<Submission, NativeError>)> {
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
    let label = adapter_label(gpu, machine);
    block_on(gpu.every_set())
        .into_iter()
        .map(|(interface, outcome)| {
            let submission = outcome.map(|signature| Submission::measured(interface, &signature, &label, seen.clone()));
            (interface, submission)
        })
        .collect()
}

/// The label of what `gpu` answers on `machine`: the adapter, the backend it was opened through, and the machine.
fn adapter_label(gpu: &Gpu, machine: &Machine) -> String {
    let (system, arch) = HERE;
    format!(
        "{} through {} on {} -- wgpu, no browser",
        gpu.info().name,
        gpu.backend(),
        machine.named(system, arch),
    )
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
