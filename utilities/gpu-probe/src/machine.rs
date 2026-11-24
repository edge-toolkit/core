//! What a row says of the machine a measurement ran on, beyond what the probes read from the device itself.
//!
//! That is the operating system's release, the CPU's model, the machine -- or, on a GitHub Actions runner, the runner
//! image and its build, since a runner label such as `macos-latest` names a pool of machines that changes under it --
//! the day, and where the run that measured it is on record.
#![expect(
    clippy::single_call_fn,
    reason = "each source of a machine's fields is a named function, read in one place"
)]

use std::collections::BTreeMap;

use sysinfo::{CpuRefreshKind, RefreshKind, System};

use crate::config::{self, Config};
use crate::registry::row;

/// What stopped the machine being described.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MachineError {
    /// The environment could not be read.
    #[error("the environment could not be read: {0}")]
    Env(#[from] serde_env::Error),
}

/// The fields a row takes from the machine a measurement ran on, each one known.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Machine {
    /// Each field's name beside its value.
    fields: BTreeMap<String, String>,
}

impl Machine {
    /// This machine, named `device` unless it is a GitHub Actions runner, which is named by its image instead.
    ///
    /// An empty `device` names the machine `GPU_PROBE_DEVICE` gives. A field whose value the system does not give is
    /// left out, as is `device` when neither names one and this is no runner.
    pub fn here(device: &str) -> Result<Self, MachineError> {
        let env = config::load()?;
        let date = jiff::Timestamp::now().to_zoned(jiff::tz::TimeZone::UTC).date();
        Ok(Self::from_parts(
            &env,
            device,
            &os_version(),
            &cpu_version(),
            &date.to_string(),
        ))
    }

    /// The machine whose fields are exactly `fields`.
    #[must_use]
    pub fn with(fields: &[(&str, &str)]) -> Self {
        Self { fields: row(fields) }
    }

    /// The machine a runner's `env` describes, or `device` off a runner, else the device `env` names.
    ///
    /// It gives the system's `release`, its `cpu`, and the `date` beside the device.
    fn from_parts(env: &Config, device: &str, release: &str, cpu: &str, date: &str) -> Self {
        let runner = (!env.imageos.is_empty())
            .then(|| format!("GitHub Actions runner image {} {}", env.imageos, env.imageversion));
        let named = if device.trim().is_empty() {
            &env.gpu.probe_device
        } else {
            device
        };
        let device = runner.unwrap_or_else(|| named.to_owned());
        let github = &env.github;
        let source = if github.run_id.is_empty() {
            String::default()
        } else {
            format!(
                "{}/{}/actions/runs/{}",
                github.server_url, github.repository, github.run_id
            )
        };
        let fields = [
            ("os_version", release),
            ("cpu_version", cpu),
            ("device", &device),
            ("date", date),
            ("source", &source),
        ];
        Self {
            fields: fields
                .into_iter()
                .map(|(field, value)| (field, value.trim()))
                .filter(|(_, value)| !value.is_empty())
                .map(|(field, value)| (field.to_owned(), value.to_owned()))
                .collect(),
        }
    }

    /// Each field this machine gives a row, its name beside its value.
    #[must_use]
    pub const fn fields(&self) -> &BTreeMap<String, String> {
        &self.fields
    }

    /// `row` with each of this machine's fields added, a field `row` gives already keeping its own value.
    #[must_use]
    pub fn row(&self, mut row: BTreeMap<String, String>) -> BTreeMap<String, String> {
        for (field, value) in &self.fields {
            let _kept: &mut String = row.entry(field.clone()).or_insert_with(|| value.clone());
        }
        row
    }

    /// The part of a label that names the machine.
    ///
    /// That is its `system`, that system's release and the CPU architecture `arch`, then the device, each where known.
    #[must_use]
    pub fn named(&self, system: &str, arch: &str) -> String {
        let field = |name: &str| self.fields.get(name).map_or("", String::as_str);
        let release = [system, field("os_version"), arch]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        [release.as_str(), field("device")]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// The operating system's release, as the registry writes it.
///
/// That is the distribution's name and version on Linux, where the name tells one distribution from another, and the
/// version alone elsewhere, where `os` already names the system.
fn os_version() -> String {
    let version = System::os_version().unwrap_or_default();
    if cfg!(target_os = "linux") {
        return [System::name().unwrap_or_default(), version].join(" ");
    }
    version
}

/// The CPU's model as the system names it, such as `Apple M5 Pro` or `AMD EPYC 7763 64-Core Processor`.
fn cpu_version() -> String {
    let refresh = RefreshKind::nothing().with_cpu(CpuRefreshKind::nothing());
    System::new_with_specifics(refresh)
        .ok()
        .and_then(|system| system.cpus().first().map(|cpu| cpu.brand().to_owned()))
        .unwrap_or_default()
}
