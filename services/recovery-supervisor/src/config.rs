//! Validated, opt-in recovery configuration with one application job per supervisor.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::RecoveryError;

/// Whether the supervisor may request workload restarts.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// Record incidents and recovery evidence without changing deployments.
    #[default]
    ObserveOnly,
    /// Request restarts within the configured budget after the grace period.
    ControllerAssisted,
}

/// Explicit timing and action bounds, in seconds measured by the supervisor's monotonic clock.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
#[serde(deny_unknown_fields)]
pub struct Policy {
    /// Continuous unhealthy observation before declaring a failure.
    pub detection_grace_seconds: u64,
    /// Minimum time between detection and an action, and between subsequent attempts.
    pub restart_after_seconds: u64,
    /// Active work without advancing progress is considered stalled after this interval.
    pub stall_seconds: u64,
    /// Maximum time from first unhealthy observation to useful recovery.
    pub deadline_seconds: u64,
    /// Lifetime restart request budget per configured workload and job, persisted before executing an action.
    pub max_restarts: u32,
}

/// One deployment and the application identity permitted to report its useful progress.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
#[serde(deny_unknown_fields)]
pub struct Workload {
    /// Kubernetes Deployment name within the configured namespace.
    pub deployment: String,
    /// Hub-assigned sender identity expected on lifecycle messages.
    pub agent_id: String,
    /// Exact `CloudEvent` source, normally the producing module's served path.
    pub source: String,
}

/// Inputs for a single supervisor; namespace and targets cannot be supplied by application messages.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Hub WebSocket URL.
    pub ws_url: String,
    /// Stable identity reserved for this supervisor.
    pub agent_id: String,
    /// Application job whose lifecycle events are eligible.
    pub job_id: String,
    /// Explicit Kubernetes context; never relies on the operator's current context.
    pub context: String,
    /// Namespace containing every allowlisted deployment.
    pub namespace: String,
    /// Observe-only unless explicitly enabled.
    #[serde(default)]
    pub mode: Mode,
    /// Time between infrastructure observation passes.
    pub poll_seconds: u64,
    /// Bound on each kubectl process, including credential-plugin execution.
    pub command_timeout_seconds: u64,
    /// Durable restart budget and latest recovery report; parent directory must exist.
    pub state_file: PathBuf,
    /// Shared recovery policy.
    pub policy: Policy,
    /// Explicit restart allowlist and sender bindings.
    pub workloads: Vec<Workload>,
}

/// Validate Kubernetes DNS identifiers without permitting flags, path fragments, or selectors.
#[must_use]
pub fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && value.starts_with(|character: char| character.is_ascii_alphanumeric())
        && value.ends_with(|character: char| character.is_ascii_alphanumeric())
}

impl Config {
    /// Reject ambiguous identities, unsafe targets, and timing settings that cannot support the policy.
    pub fn validate(&self) -> Result<(), RecoveryError> {
        if self.context.is_empty()
            || self.context.starts_with('-')
            || !valid_name(&self.namespace)
            || self.agent_id.is_empty()
            || self.job_id.is_empty()
            || self.state_file.as_os_str().is_empty()
            || !(self.ws_url.starts_with("ws://") || self.ws_url.starts_with("wss://"))
        {
            return Err(RecoveryError::Invalid(
                "invalid hub, job, namespace, context, or state location".into(),
            ));
        }
        let policy = &self.policy;
        if [
            self.poll_seconds,
            self.command_timeout_seconds,
            policy.detection_grace_seconds,
            policy.restart_after_seconds,
            policy.stall_seconds,
            policy.deadline_seconds,
        ]
        .contains(&0)
            || policy.deadline_seconds
                <= policy
                    .detection_grace_seconds
                    .saturating_add(policy.restart_after_seconds)
            || self.workloads.is_empty()
        {
            return Err(RecoveryError::Invalid(
                "recovery timings must be positive and permit detection followed by recovery".into(),
            ));
        }
        let mut agents = BTreeSet::new();
        let mut deployments = BTreeSet::new();
        for workload in &self.workloads {
            if !valid_name(&workload.deployment)
                || workload.agent_id.is_empty()
                || workload.agent_id == self.agent_id
                || !workload.source.starts_with("/modules/")
                || !agents.insert(&workload.agent_id)
                || !deployments.insert(&workload.deployment)
            {
                return Err(RecoveryError::Invalid(
                    "workload targets and sender identities must be valid and unique".into(),
                ));
            }
        }
        Ok(())
    }
}
