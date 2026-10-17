//! Per-process resource limits a scenario can set, which every container format enforces identically.
//!
//! The set is what compose enforces on a cgroup v2 host, which is every host this project supports and the only kind
//! k3s runs on. Compose also accepts `mem_swappiness`, `oom_kill_disable` and the `cpu_rt_*` pair, but Docker discards
//! the first two there with a warning and refuses the third outright, so they are not features a scenario could rely
//! on. Per-device block I/O limits and device reservations name host devices, which a scenario written once for every
//! machine cannot know. `oom_score_adj` is a property of the process rather than of its cgroup, and the kubelet sets
//! it from each pod's `QoS` class, so no container runtime plugin can hold it to a scenario's value.
//!
//! Each field resolves to the same cgroup value in both formats, even where they spell it differently. k3s applies the
//! ones the Pod API carries natively and has two container runtime plugins write the rest straight into the
//! container's cgroup, so a field never means one thing under compose and another under k3s. The mise and dekit
//! deployments run each process directly on the host, outside any container, and apply none of them.

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::num::NonZeroU64;

use bytesize::ByteSize;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::deployment_types::{COLLECTOR_TASK, HUB_TASK};
use crate::error::CliError;
use crate::input::{ClusterInput, OutputType};

/// The CPU limit a process runs under when its scenario sets none.
///
/// Kubernetes lets a container with no limit consume the node, so every process gets one; generous rather than tuned,
/// because a deployment of a scenario is a demonstration and a limit that throttles the runners would turn a passing
/// exchange into a flaky one. Compose is held to the same figure so that the two run the same process the same way.
const CPU_LIMIT: Cpus = Cpus::from_millicores(2000);
/// The CPU reservation a process runs under when its scenario sets none, which sets its CPU weight in both formats.
const CPU_REQUEST: Cpus = Cpus::from_millicores(100);
/// The memory limit a process runs under when its scenario sets none.
const MEMORY_LIMIT: ByteSize = ByteSize::gib(2);
/// The memory a Kubernetes scheduler sets aside for a process whose scenario reserves none.
///
/// Only the scheduler reads this: a request alone puts nothing in the container's cgroup, so compose, which has no
/// scheduler, has nothing to match.
const MEMORY_REQUEST: ByteSize = ByteSize::mib(128);
/// The period, in microseconds, a CPU quota is measured over when the scenario names none.
///
/// The kernel's own default, and the one Kubernetes measures every CPU limit over.
const CPU_PERIOD: u64 = 100_000;
/// The largest CPU weight Docker accepts, in shares; the kernel's own cgroup v1 ceiling, 2^18.
pub(crate) const MAX_CPU_SHARES: u64 = 1 << 18_u32;

/// What is wrong with a process's limits: a value Docker would refuse when the container starts.
#[derive(Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum LimitsError {
    #[error("{0:?} is not a positive number of CPUs with at most three decimal places")]
    InvalidCpus(String),

    #[error("cpu_reservation {reservation} exceeds cpus {limit}")]
    CpuReservationAboveLimit { reservation: Cpus, limit: Cpus },

    #[error("memory_reservation {reservation} exceeds memory {limit}")]
    MemoryReservationAboveLimit { reservation: ByteSize, limit: ByteSize },

    #[error("memswap_limit {swap} is below memory {memory}")]
    SwapBelowMemory { swap: ByteSize, memory: ByteSize },

    #[error("{field} must be from {min} to {max}")]
    OutOfRange { field: &'static str, min: u64, max: u64 },

    #[error("{first} and {second} set the same control, so only one of them may be given")]
    Conflict { first: &'static str, second: &'static str },

    #[error("cpu_period needs a cpu_quota to measure")]
    PeriodWithoutQuota,

    #[error("cpuset {0:?} must list CPU numbers and ranges, e.g. 0-3 or 0,2")]
    InvalidCpuset(String),

    #[error("ulimit name {0:?} must be lowercase letters, e.g. nofile")]
    InvalidUlimitName(String),

    #[error("ulimit {name} has soft limit {soft} above its hard limit {hard}")]
    UlimitSoftAboveHard { name: String, soft: u64, hard: u64 },
}

/// A CPU amount in whole millicores, read from a decimal such as `0.5` or `2`.
///
/// Held as an integer so neither format renders a float: compose takes the decimal back, Kubernetes takes millicores.
/// More than three decimal places would be a fraction of a millicore, which neither format can express.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Cpus(u32);

impl Cpus {
    /// An amount of `millicores` thousandths of a CPU.
    #[must_use]
    pub const fn from_millicores(millicores: u32) -> Self {
        Self(millicores)
    }

    /// The amount in millicores.
    #[must_use]
    pub const fn millicores(self) -> u32 {
        self.0
    }

    /// The CPU weight this amount reserves, in the shares the kernel's cgroup v1 interface and Docker speak.
    ///
    /// The kubelet's own conversion of a request, `millicores * 1024 / 1000` with a floor of two shares, so a
    /// compose process reserving this amount gets the weight a Kubernetes container requesting it does.
    #[must_use]
    pub fn shares(self) -> u64 {
        (u64::from(self.0).saturating_mul(1024).div_euclid(1000)).max(2)
    }

    /// Parse a positive decimal with at most three places, e.g. `0.25`.
    fn parse(raw: &str) -> Result<Self, LimitsError> {
        let invalid = || LimitsError::InvalidCpus(raw.to_owned());
        let trimmed = raw.trim();
        let (whole, fraction) = trimmed.split_once('.').unwrap_or((trimmed, ""));
        if whole.is_empty()
            || fraction.len() > 3
            || !(whole
                .chars()
                .chain(fraction.chars())
                .all(|digit| digit.is_ascii_digit()))
        {
            return Err(invalid());
        }
        // Both halves are all digits by now, so the only parse failure left is a whole part too large for a `u32`.
        let (Ok(whole), Ok(fraction)) = (whole.parse::<u32>(), format!("{fraction:0<3}").parse::<u32>()) else {
            return Err(invalid());
        };
        let millicores = whole.checked_mul(1000).and_then(|cores| cores.checked_add(fraction));
        match millicores {
            Some(0) | None => Err(invalid()),
            Some(millicores) => Ok(Self(millicores)),
        }
    }
}

impl fmt::Display for Cpus {
    /// The decimal compose reads, without trailing zeros: `0.5`, `2`, `1.25`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let whole = self.0.div_euclid(1000);
        let fraction = self.0.rem_euclid(1000);
        if fraction == 0 {
            return write!(f, "{whole}");
        }
        let digits = format!("{fraction:03}");
        write!(f, "{whole}.{}", digits.trim_end_matches('0'))
    }
}

impl<'de> Deserialize<'de> for Cpus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        /// A YAML number or string, which both spell a CPU amount.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Number(serde_yaml::Number),
            Text(String),
        }
        let raw = match Raw::deserialize(deserializer)? {
            Raw::Number(number) => number.to_string(),
            Raw::Text(text) => text,
        };
        match Self::parse(&raw) {
            Ok(cpus) => Ok(cpus),
            Err(error) => Err(serde::de::Error::custom(error)),
        }
    }
}

impl Serialize for Cpus {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}

/// Write a size as its exact count of bytes.
///
/// `ByteSize`'s own human-readable form rounds to one decimal place of its unit, so a size read back from it can differ
/// from the one written.
#[expect(
    clippy::ref_option,
    reason = "serde's `serialize_with` hands the function a reference to the field, which is an `Option`"
)]
fn serialize_bytes<S>(size: &Option<ByteSize>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    size.map(|size| size.as_u64()).serialize(serializer)
}

/// One `ulimits` entry: a single value for both the soft and hard limit, or the two separately.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum Ulimit {
    /// The same value for the soft and the hard limit.
    Both(u64),
    /// The soft limit and the hard limit it may be raised to.
    Pair { soft: u64, hard: u64 },
}

impl Ulimit {
    /// The soft and hard limit, in that order.
    #[must_use]
    pub const fn soft_hard(self) -> (u64, u64) {
        match self {
            Self::Both(value) => (value, value),
            Self::Pair { soft, hard } => (soft, hard),
        }
    }
}

/// The resource limits one process runs under, every one optional.
///
/// An unset field takes the default every process has, which is what a scenario that sets none has always run with
/// under k3s; that absence is the whole point of each field being an `Option`.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Limits {
    /// Block I/O weight relative to other containers, from 10 to 1000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blkio_weight: Option<u16>,
    /// Length, in microseconds, of the period `cpu_quota` is measured over, from 1000 to 1000000; 100000 by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_period: Option<u64>,
    /// CPU time, in microseconds, the process may use per `cpu_period`; the alternative to `cpus`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_quota: Option<u64>,
    /// CPUs held for the process, e.g. `0.25`, which sets its CPU weight; the alternative to `cpu_shares`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_reservation: Option<Cpus>,
    /// CPU weight relative to other containers, from 2 to 262144, where 1024 is one CPU's worth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_shares: Option<u64>,
    /// CPUs the process may use at most, e.g. `0.5` or `2`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpus: Option<Cpus>,
    /// CPUs the process may run on, e.g. `0-3` or `0,2`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpuset: Option<String>,
    /// Memory the process may use at most, e.g. `512MiB`.
    #[serde(default, serialize_with = "serialize_bytes", skip_serializing_if = "Option::is_none")]
    pub memory: Option<ByteSize>,
    /// Memory the kernel protects from reclaim for the process, e.g. `128MiB`.
    #[serde(default, serialize_with = "serialize_bytes", skip_serializing_if = "Option::is_none")]
    pub memory_reservation: Option<ByteSize>,
    /// Memory plus swap the process may use at most; at least its memory limit.
    #[serde(default, serialize_with = "serialize_bytes", skip_serializing_if = "Option::is_none")]
    pub memswap_limit: Option<ByteSize>,
    /// Processes and threads the process may run at most.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pids: Option<NonZeroU64>,
    /// Size of the process's `/dev/shm`.
    #[serde(default, serialize_with = "serialize_bytes", skip_serializing_if = "Option::is_none")]
    pub shm_size: Option<ByteSize>,
    /// Kernel resource limits by name, e.g. `nofile`, each one value or a `soft`/`hard` pair.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ulimits: BTreeMap<String, Ulimit>,
}

/// How a process's CPU time is capped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CpuCap {
    /// A number of CPUs, which both formats turn into a quota over the default period.
    Cpus(Cpus),
    /// An explicit quota per period, both in microseconds.
    Quota { quota: u64, period: u64 },
}

impl CpuCap {
    /// The cap in millicores, rounded up, for the places that can only state a CPU amount.
    #[must_use]
    pub fn millicores(self) -> Cpus {
        match self {
            Self::Cpus(cpus) => cpus,
            Self::Quota { quota, period } => {
                let millicores = quota.saturating_mul(1000).div_ceil(period);
                Cpus::from_millicores(u32::try_from(millicores).unwrap_or(u32::MAX))
            }
        }
    }
}

/// A process's limits with every default applied: the figures both container formats render.
///
/// Resolving once, here, is what keeps the formats from drifting apart. Each default gives way to a value the scenario
/// did set, so a request never ends up above its limit: an unset reservation is lowered to a small limit, and an unset
/// limit raised to a large reservation.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ResolvedLimits {
    /// The CPU cap.
    pub cpu_cap: CpuCap,
    /// The CPU amount reserved for scheduling, from `cpu_reservation` or the default.
    pub cpu_request: Cpus,
    /// The CPU weight, in shares: `cpu_shares` when set, otherwise what `cpu_request` reserves.
    pub cpu_shares: u64,
    /// Whether `cpu_shares` came from the scenario rather than from the reservation.
    pub explicit_shares: bool,
    /// The memory limit.
    pub memory: ByteSize,
    /// The memory set aside for scheduling.
    pub memory_request: ByteSize,
    /// The scenario's `memory_reservation`, protected from reclaim; absent, the kernel protects nothing.
    pub memory_low: Option<ByteSize>,
    /// The scenario's other limits, applied as they are.
    pub rest: Limits,
}

impl Default for ResolvedLimits {
    /// The defaults alone: what a process the scenario sets no limits for runs under.
    fn default() -> Self {
        Self {
            cpu_cap: CpuCap::Cpus(CPU_LIMIT),
            cpu_request: CPU_REQUEST,
            cpu_shares: CPU_REQUEST.shares(),
            explicit_shares: false,
            memory: MEMORY_LIMIT,
            memory_request: MEMORY_REQUEST,
            memory_low: None,
            rest: Limits::default(),
        }
    }
}

impl Limits {
    /// Whether no limit is set at all, which is how a scenario that predates limits reads.
    #[must_use]
    pub fn is_unset(&self) -> bool {
        *self == Self::default()
    }

    /// These limits with every default applied.
    #[must_use]
    pub fn resolve(&self) -> ResolvedLimits {
        let cpu_cap = match (self.cpus, self.cpu_quota) {
            (_, Some(quota)) => CpuCap::Quota {
                quota,
                period: self.cpu_period.unwrap_or(CPU_PERIOD),
            },
            (Some(cpus), None) => CpuCap::Cpus(cpus),
            (None, None) => CpuCap::Cpus(self.cpu_reservation.map_or(CPU_LIMIT, |request| request.max(CPU_LIMIT))),
        };
        let cpu_request = self
            .cpu_reservation
            .unwrap_or_else(|| CPU_REQUEST.min(cpu_cap.millicores()));
        let memory = self.memory.unwrap_or_else(|| {
            self.memory_reservation
                .map_or(MEMORY_LIMIT, |request| request.max(MEMORY_LIMIT))
        });
        ResolvedLimits {
            cpu_cap,
            cpu_request,
            cpu_shares: self.cpu_shares.unwrap_or_else(|| cpu_request.shares()),
            explicit_shares: self.cpu_shares.is_some(),
            memory,
            memory_request: self.memory_reservation.unwrap_or_else(|| MEMORY_REQUEST.min(memory)),
            memory_low: self.memory_reservation,
            rest: self.clone(),
        }
    }
}

/// Reject limits Docker would refuse when the container starts, naming the process and the reason.
///
/// Caught while the scenario is read, so a mistake fails generation rather than a deployment on some later machine.
pub(crate) fn validate_limits(process: &str, limits: &Limits) -> Result<(), CliError> {
    match check_limits(limits) {
        Ok(()) => Ok(()),
        Err(reason) => Err(CliError::InvalidLimits {
            process: process.to_owned(),
            reason,
        }),
    }
}

/// Check `limits` against what Docker accepts, failing with the first thing wrong.
#[expect(
    clippy::single_call_fn,
    reason = "the checks behind validate_limits, kept apart so each reads as one condition per line"
)]
fn check_limits(limits: &Limits) -> Result<(), LimitsError> {
    check_cpu(limits)?;
    if let (Some(reservation), Some(limit)) = (limits.memory_reservation, limits.memory)
        && reservation > limit
    {
        return Err(LimitsError::MemoryReservationAboveLimit { reservation, limit });
    }
    let memory = limits.resolve().memory;
    if let Some(swap) = limits.memswap_limit
        && swap < memory
    {
        return Err(LimitsError::SwapBelowMemory { swap, memory });
    }
    if let Some(cpuset) = &limits.cpuset
        && !cpuset.split(',').all(cpuset_item_is_valid)
    {
        return Err(LimitsError::InvalidCpuset(cpuset.clone()));
    }
    limits
        .ulimits
        .iter()
        .try_for_each(|(name, ulimit)| check_ulimit(name, *ulimit))
}

/// Check the CPU controls: one way of capping and one way of weighting, each within the range Docker accepts.
fn check_cpu(limits: &Limits) -> Result<(), LimitsError> {
    if let (Some(reservation), Some(limit)) = (limits.cpu_reservation, limits.cpus)
        && reservation > limit
    {
        return Err(LimitsError::CpuReservationAboveLimit { reservation, limit });
    }
    let conflicts = [
        ("cpus", limits.cpus.is_some(), "cpu_quota", limits.cpu_quota.is_some()),
        ("cpus", limits.cpus.is_some(), "cpu_period", limits.cpu_period.is_some()),
        (
            "cpu_reservation",
            limits.cpu_reservation.is_some(),
            "cpu_shares",
            limits.cpu_shares.is_some(),
        ),
    ];
    for (first, first_set, second, second_set) in conflicts {
        if first_set && second_set {
            return Err(LimitsError::Conflict { first, second });
        }
    }
    if limits.cpu_period.is_some() && limits.cpu_quota.is_none() {
        return Err(LimitsError::PeriodWithoutQuota);
    }
    let ranges = [
        ("blkio_weight", limits.blkio_weight.map(u64::from), 10_u64, 1_000_u64),
        ("cpu_period", limits.cpu_period, 1_000_u64, 1_000_000_u64),
        ("cpu_quota", limits.cpu_quota, 1_000_u64, u64::MAX),
        ("cpu_shares", limits.cpu_shares, 2_u64, MAX_CPU_SHARES),
    ];
    for (field, value, min, max) in ranges {
        if value.is_some_and(|value| !(min..=max).contains(&value)) {
            return Err(LimitsError::OutOfRange { field, min, max });
        }
    }
    Ok(())
}

/// Check one `ulimits` entry has a kernel-style name and a soft limit no higher than its hard one.
fn check_ulimit(name: &str, ulimit: Ulimit) -> Result<(), LimitsError> {
    if name.is_empty() || !name.chars().all(|letter| letter.is_ascii_lowercase()) {
        return Err(LimitsError::InvalidUlimitName(name.to_owned()));
    }
    let (soft, hard) = ulimit.soft_hard();
    if soft > hard {
        return Err(LimitsError::UlimitSoftAboveHard {
            name: name.to_owned(),
            soft,
            hard,
        });
    }
    Ok(())
}

/// Whether one comma-separated `cpuset` item is a CPU number or an ascending inclusive range of them.
///
/// The grammar both Docker's `--cpuset-cpus` and the kernel's `cpuset.cpus` accept, so an item either would refuse at
/// start -- empty, a dangling `-`, a descending range -- is caught while the scenario is read.
fn cpuset_item_is_valid(item: &str) -> bool {
    // A single CPU is the range from itself to itself.
    let (first, last) = item.split_once('-').unwrap_or((item, item));
    let digits = |number: &str| !number.is_empty() && number.bytes().all(|digit| digit.is_ascii_digit());
    digits(first)
        && digits(last)
        && matches!((first.parse::<u32>(), last.parse::<u32>()), (Ok(first), Ok(last)) if first <= last)
}

/// A byte size as compose writes one: the largest binary unit that divides it, `512m` or `2g`.
#[must_use]
pub(crate) fn compose_bytes(size: ByteSize) -> String {
    binary_units(size, ["g", "m", "k", "b"])
}

/// A byte size as a Kubernetes quantity: the largest binary unit that divides it, `512Mi` or `2Gi`.
#[must_use]
pub(crate) fn k8s_bytes(size: ByteSize) -> String {
    binary_units(size, ["Gi", "Mi", "Ki", ""])
}

/// Render `size` in the largest of GiB, MiB and KiB that divides it exactly, else in bytes.
fn binary_units(size: ByteSize, suffixes: [&str; 4]) -> String {
    let bytes = size.as_u64();
    for (scale, suffix) in [1 << 30_u32, 1 << 20_u32, 1 << 10_u32].into_iter().zip(suffixes) {
        if bytes != 0 && bytes.is_multiple_of(scale) {
            return format!("{}{suffix}", bytes.div_euclid(scale));
        }
    }
    format!("{bytes}{}", suffixes[3])
}

/// A CPU amount as a Kubernetes quantity: whole cores as an integer, anything else in millicores.
#[must_use]
pub(crate) fn k8s_cpus(cpus: Cpus) -> String {
    let millicores = cpus.millicores();
    if millicores.is_multiple_of(1000) {
        format!("{}", millicores.div_euclid(1000))
    } else {
        format!("{millicores}m")
    }
}

/// Every process the scenario sets limits for, by the name each deployment format gives it.
fn limited_processes(cluster: &ClusterInput) -> Vec<&str> {
    let fixed = [
        (COLLECTOR_TASK, &cluster.services.collector.limits),
        (HUB_TASK, &cluster.services.hub.limits),
    ];
    let agents = cluster.agents.iter().map(|agent| (agent.name.as_str(), &agent.limits));
    fixed
        .into_iter()
        .chain(agents)
        .filter(|(_, limits)| !limits.is_unset())
        .map(|(process, _)| process)
        .collect()
}

/// Render that a format which runs processes on the host applies none of the scenario's limits, or nothing otherwise.
///
/// Said in the README rather than left out of the output silently, so an operator relying on a limit learns which
/// deployments do not hold it.
pub(crate) fn limits_note(output_type: OutputType, cluster: &ClusterInput) -> String {
    // Named per format, because every format's section lands in the one README and markdownlint rejects a repeated
    // heading.
    let heading = match output_type {
        OutputType::DockerCompose | OutputType::K3s => return String::default(),
        OutputType::Mise => "Mise",
        OutputType::Dekit => "dekit",
        OutputType::ProcessCompose => "process-compose",
    };
    let processes = limited_processes(cluster);
    if processes.is_empty() {
        return String::default();
    }
    let lines = processes.iter().fold(String::default(), |mut lines, process| {
        let _write_result = writeln!(lines, "- `{process}`");
        lines
    });
    format!(
        concat!(
            "\n### Limits Not Applied With {heading}\n\n",
            "Each process runs directly on the host rather than in a container, so the limits the scenario\n",
            "sets for these are not applied; the compose and k3s deployments enforce them:\n\n",
            "{lines}"
        ),
        heading = heading,
        lines = lines,
    )
}
