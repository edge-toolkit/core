//! Optional recovery coordination and explicit, evidence-based decision reporting.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{DefaultHasher, Hasher as _};

use serde::{Deserialize, Serialize};

use crate::RecoveryError;
use crate::config::{Config, valid_name};
use crate::policy::ObservationTime;

/// Additional safeguards; zero-valued limits preserve unrestricted fleet coordination.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "env-schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct Settings {
    /// Retry a lost supervisor connection rather than exit; no interventions occur while disconnected.
    pub reconnect: bool,
    /// Bound each connect/register attempt; the shared transport supplies its retry backoff.
    pub connect_timeout_seconds: u64,
    /// Maximum workloads with an outstanding recovery request; zero disables this additional limit.
    pub max_concurrent_recoveries: u32,
    /// Minimum interval between restart reservations across all workloads; zero disables the limit.
    pub action_interval_seconds: u64,
    /// Multiply the per-workload cooldown after successive attempts; one preserves a fixed cooldown.
    pub backoff_multiplier: u32,
    /// Upper bound on the cooldown including jitter; never shortens the original cooldown.
    pub backoff_max_seconds: u64,
    /// Random additional cooldown, bounded by `backoff_max_seconds`; zero disables jitter.
    pub jitter_seconds: u64,
    /// Healthy observation window after first useful recovery; zero disables stability assessment.
    pub stability_seconds: u64,
    /// Required Deployment dependencies per supervised workload; dependencies are observed, never restarted.
    pub dependencies: BTreeMap<String, Vec<String>>,
}

impl Settings {
    /// Reject unsafe dependency names, cycles and cooldown bounds that weaken the base policy.
    pub fn validate(&self, config: &Config) -> Result<(), RecoveryError> {
        self.validate_timings()?;
        for (workload, dependencies) in &self.dependencies {
            if !config.workloads.iter().any(|item| &item.deployment == workload) {
                return Err(RecoveryError::Invalid(
                    "dependency owner is not a supervised workload".into(),
                ));
            }
            let unique: BTreeSet<_> = dependencies.iter().collect();
            if unique.len() != dependencies.len()
                || dependencies.iter().any(|name| !valid_name(name) || name == workload)
            {
                return Err(RecoveryError::Invalid(
                    "dependencies must have unique, valid Deployment names".into(),
                ));
            }
            self.validate_path(workload, &mut BTreeSet::new())?;
        }
        Ok(())
    }

    /// Check retry timing independently of the deployment dependency graph.
    fn validate_timings(&self) -> Result<(), RecoveryError> {
        if self.connect_timeout_seconds == 0
            || !(1..=16).contains(&self.backoff_multiplier)
            || self.backoff_max_seconds == 0
            || self.jitter_seconds > self.backoff_max_seconds
        {
            return Err(RecoveryError::Invalid(
                "invalid resilience timing or backoff bounds".into(),
            ));
        }
        Ok(())
    }

    /// Validate a path rather than a global visited set so shared dependencies remain valid.
    fn validate_path(&self, workload: &str, path: &mut BTreeSet<String>) -> Result<(), RecoveryError> {
        if !path.insert(workload.into()) {
            return Err(RecoveryError::Invalid("recovery dependencies contain a cycle".into()));
        }
        if let Some(dependencies) = self.dependencies.get(workload) {
            for dependency in dependencies {
                self.validate_path(dependency, path)?;
            }
        }
        let _removed = path.remove(workload);
        Ok(())
    }

    /// Return all additional read-only deployment targets in a deterministic order.
    #[must_use]
    pub fn dependency_names(&self) -> BTreeSet<String> {
        self.dependencies.values().flatten().cloned().collect()
    }

    /// Derive a bounded delay with a persisted per-supervisor seed and a stable per-attempt sample.
    #[must_use]
    pub fn cooldown(&self, base: u64, attempts: u32, workload: &str, seed: u64) -> u64 {
        if attempts == 0 {
            return base;
        }
        let exponent = attempts.saturating_sub(1).min(64);
        let multiplier = u64::from(self.backoff_multiplier).saturating_pow(exponent);
        let delay = base.saturating_mul(multiplier).min(self.backoff_max_seconds);
        let mut sample = DefaultHasher::new();
        sample.write_u64(seed);
        sample.write(workload.as_bytes());
        sample.write_u32(attempts);
        let jitter = sample
            .finish()
            .checked_rem(self.jitter_seconds.saturating_add(1))
            .unwrap_or(0);
        // skipcq: RS-W1069 -- base can exceed the cap; preserve it without clamp's inverted-bound panic
        delay.saturating_add(jitter).min(self.backoff_max_seconds).max(base)
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            reconnect: true,
            connect_timeout_seconds: 30,
            max_concurrent_recoveries: 0,
            action_interval_seconds: 0,
            backoff_multiplier: 1,
            backoff_max_seconds: 300,
            jitter_seconds: 0,
            stability_seconds: 0,
            dependencies: BTreeMap::new(),
        }
    }
}

/// Accept the same settings as a scenario mapping or a single JSON environment value.
pub fn deserialize<'de, D>(deserializer: D) -> Result<Settings, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Input {
        Encoded(String),
        Structured(Settings),
    }
    match Input::deserialize(deserializer)? {
        Input::Encoded(value) => match serde_json::from_str(&value) {
            Ok(settings) => Ok(settings),
            Err(error) => Err(serde::de::Error::custom(error)),
        },
        Input::Structured(settings) => Ok(settings),
    }
}

/// Observed condition, not a speculative diagnosis of hardware or application internals.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Classification {
    Healthy,
    DeploymentUnavailable,
    ApplicationFailure,
    ProgressStalled,
    DependencyUnavailable,
    ObservationUnknown,
}

/// Why policy permits or suppresses an intervention at its most recent assessment.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Reason {
    Healthy,
    ObservationUnknown,
    EvidenceRequired,
    DependencyUnavailable,
    DependencyUnknown,
    DetectionGrace,
    ObserveOnly,
    Cooldown,
    ConcurrencyLimit,
    ActionInterval,
    BudgetExhausted,
    DeadlineExceeded,
    RestartEligible,
    RecoveryInProgress,
}

/// Latest assessment per workload; bounded by the configured workload set.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct Assessment {
    pub classification: Classification,
    pub reason: Reason,
    pub observed_at: u64,
    pub next_action_at: ObservationTime,
    pub dependencies: Vec<String>,
}

/// Sustained recovery is measured separately from the first useful recovery and MTTR.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Stability {
    #[default]
    Disabled,
    Pending {
        since: u64,
        progress: u64,
        instance: String,
    },
    Stable {
        verified_at: u64,
    },
    Unstable {
        observed_at: u64,
    },
    Incomplete,
}

/// Explicit periods when application health could not be observed.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct ObservationGap {
    pub started_at: u64,
    pub ended_at: ObservationTime,
    pub reason: String,
}
