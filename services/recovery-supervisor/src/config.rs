//! Validated, opt-in recovery configuration with one application job per supervisor.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::RecoveryError;

/// Whether the supervisor may request workload restarts.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "env-schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "env-schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "env-schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "env-schema", derive(schemars::JsonSchema))]
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
    #[serde(deserialize_with = "deserialize_workloads")]
    #[cfg_attr(feature = "env-schema", schemars(with = "String"))]
    pub workloads: Vec<Workload>,
}

/// Environment configuration grouped under the `RECOVERY_*` prefix.
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "env-schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct EnvConfig {
    /// Recovery connection settings, bounded policy, and workload allowlist.
    pub recovery: Config,
}

/// Decode the environment's JSON workload list while preserving structured configuration round trips.
fn deserialize_workloads<'de, D>(deserializer: D) -> Result<Vec<Workload>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Input {
        Encoded(String),
        Structured(Vec<Workload>),
    }
    match Input::deserialize(deserializer)? {
        Input::Encoded(value) => match serde_json::from_str(&value) {
            Ok(workloads) => Ok(workloads),
            Err(error) => Err(serde::de::Error::custom(error)),
        },
        Input::Structured(value) => Ok(value),
    }
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
    /// Build in-cluster settings with observe-only mode, two-second polling and ten-second command deadlines.
    ///
    /// Customize these defaults as needed and validate the completed settings before starting supervision.
    #[must_use]
    pub fn in_cluster(
        ws_url: String,
        namespace: String,
        job_id: String,
        policy: Policy,
        workloads: Vec<Workload>,
    ) -> Self {
        Self {
            ws_url,
            agent_id: "recovery-supervisor".into(),
            job_id,
            context: "in-cluster".into(),
            namespace,
            mode: Mode::ObserveOnly,
            poll_seconds: 2,
            command_timeout_seconds: 10,
            state_file: PathBuf::from("/app/state/recovery.json"),
            policy,
            workloads,
        }
    }

    /// Reject ambiguous identities, unsafe targets, and timing settings that cannot support the policy.
    pub fn validate(&self) -> Result<(), RecoveryError> {
        self.validate_identity()?;
        self.policy.validate()?;
        if self.poll_seconds == 0 || self.command_timeout_seconds == 0 || self.workloads.is_empty() {
            return Err(RecoveryError::Invalid(
                "positive polling and command deadlines and at least one workload are required".into(),
            ));
        }
        let mut agents = BTreeSet::new();
        let mut deployments = BTreeSet::new();
        for workload in &self.workloads {
            workload.validate(&self.agent_id)?;
            if !agents.insert(&workload.agent_id) || !deployments.insert(&workload.deployment) {
                return Err(RecoveryError::Invalid(
                    "workload targets and sender identities must be unique".into(),
                ));
            }
        }
        Ok(())
    }

    /// Reject ambiguous job, deployment namespace and durable-state identities before connecting.
    fn validate_identity(&self) -> Result<(), RecoveryError> {
        if self.context.is_empty()
            || self.context.starts_with('-')
            || !valid_name(&self.namespace)
            || self.agent_id.is_empty()
            || self.job_id.is_empty()
            || self.state_file.as_os_str().is_empty()
        {
            return Err(RecoveryError::Invalid(
                "invalid hub, job, namespace, context, or state location".into(),
            ));
        }
        validate_endpoint(&self.ws_url)
    }
}

/// Validate transport syntax without forbidding trusted-cluster deployments that terminate TLS elsewhere.
#[expect(
    clippy::single_call_fn,
    reason = "connection syntax is independent of recovery identity and timing"
)]
fn validate_endpoint(value: &str) -> Result<(), RecoveryError> {
    let endpoint = url::Url::parse(value)?;
    // Plain WebSockets support trusted cluster networks; use TLS across untrusted network boundaries.
    if !["ws", "wss"].contains(&endpoint.scheme())
        || endpoint.host_str().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(RecoveryError::Invalid("invalid recovery WebSocket endpoint".into()));
    }
    Ok(())
}

impl Policy {
    /// Validate positive recovery timings and a deadline that permits recovery.
    fn validate(&self) -> Result<(), RecoveryError> {
        if [
            self.detection_grace_seconds,
            self.restart_after_seconds,
            self.stall_seconds,
            self.deadline_seconds,
        ]
        .contains(&0)
            || self.deadline_seconds <= self.detection_grace_seconds.saturating_add(self.restart_after_seconds)
        {
            return Err(RecoveryError::Invalid(
                "recovery timings must be positive and permit detection followed by recovery".into(),
            ));
        }
        Ok(())
    }
}

impl Workload {
    /// Validate one deployment target and its permitted lifecycle sender.
    fn validate(&self, supervisor: &str) -> Result<(), RecoveryError> {
        if !valid_name(&self.deployment)
            || self.agent_id.is_empty()
            || self.agent_id == supervisor
            || !self.source.starts_with("/modules/")
        {
            return Err(RecoveryError::Invalid(
                "workload target and sender identity must be valid".into(),
            ));
        }
        Ok(())
    }
}
