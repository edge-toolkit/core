//! Optional scenario recovery configuration, validated against the actual runner deployments before writing output.

use std::collections::BTreeMap;
use std::path::Path;

use et_recovery_supervisor::config::{Config, Mode, Policy, Workload};
use fs_err as fs;
use serde::{Deserialize, Serialize};

use crate::limits::Limits;
use crate::{CliError, ClusterInput, OutputType, RunnerInstance, hub_service_ws_url};

/// Absence is a normal scenario without recovery, not an implicit policy or permission grant.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum Recovery {
    /// Explicit recovery settings.
    Configured(Box<RecoverySettings>),
    /// No supervisor is deployed.
    #[default]
    Disabled,
}

impl Recovery {
    /// Whether recovery was omitted from the scenario.
    #[must_use]
    pub const fn is_unset(&self) -> bool {
        matches!(self, Self::Disabled)
    }
}

impl<'de> Deserialize<'de> for Recovery {
    /// Preserve detailed errors when decoding optional scenario recovery settings.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(RecoveryVisitor)
    }
}

/// Preserve field-specific configuration errors rather than hiding them behind untagged-enum matching.
struct RecoveryVisitor;

impl<'de> serde::de::Visitor<'de> for RecoveryVisitor {
    type Value = Recovery;

    /// Describe the accepted mapping-or-null shape in configuration errors.
    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a recovery configuration mapping or null")
    }

    /// Treat an explicit null recovery block as disabled supervision.
    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(Recovery::Disabled)
    }

    /// Decode an explicit recovery mapping without hiding field-specific validation errors.
    fn visit_map<M>(self, map: M) -> Result<Self::Value, M::Error>
    where
        M: serde::de::MapAccess<'de>,
    {
        let settings = RecoverySettings::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
        Ok(Recovery::Configured(Box::new(settings)))
    }
}

/// Recovery policy and application bindings; Kubernetes credentials and storage paths are generated, not user input.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct RecoverySettings {
    /// Job identity emitted by all participating application modules.
    pub job_id: String,
    /// Observation is the default; intervention must be explicitly enabled.
    #[serde(default)]
    pub mode: Mode,
    /// Infrastructure observation interval.
    pub poll_seconds: u64,
    /// Deadline for one Kubernetes command.
    pub command_timeout_seconds: u64,
    /// Shared, bounded recovery policy.
    pub policy: Policy,
    /// Existing, continuous native-Python deployments and their application evidence identities.
    pub workloads: Vec<Workload>,
    /// Optional safeguards for transport, dependencies, coordinated actions and sustained recovery.
    #[serde(default)]
    pub resilience: et_recovery_supervisor::resilience::Settings,
    /// Limits for the supervisor's own container.
    #[serde(default, skip_serializing_if = "Limits::is_unset")]
    pub limits: Limits,
}

impl RecoverySettings {
    /// Construct the same validated runtime configuration a standalone operator supplies through the environment.
    pub fn config(&self, namespace: &str) -> Result<Config, CliError> {
        let mut config = Config::in_cluster(
            hub_service_ws_url(),
            namespace.into(),
            self.job_id.clone(),
            self.policy.clone(),
            self.workloads.clone(),
        );
        config.mode = self.mode;
        config.poll_seconds = self.poll_seconds;
        config.command_timeout_seconds = self.command_timeout_seconds;
        config.resilience.clone_from(&self.resilience);
        config.validate()?;
        Ok(config)
    }

    /// Environment variables consumed by the supervisor image.
    pub fn environment(&self, namespace: &str) -> Result<BTreeMap<String, String>, CliError> {
        let config = self.config(namespace)?;
        let mut env = BTreeMap::new();
        let values: BTreeMap<String, serde_json::Value> = serde_json::from_value(serde_json::to_value(config)?)?;
        for (key, value) in values {
            if key == "policy" {
                let policy: BTreeMap<String, serde_json::Value> = serde_json::from_value(value)?;
                for (setting, number) in policy {
                    let _previous = env.insert(
                        format!("RECOVERY_POLICY_{}", setting.to_uppercase()),
                        number.to_string(),
                    );
                }
            } else {
                let text = value.as_str().map_or_else(|| value.to_string(), str::to_owned);
                let _previous = env.insert(format!("RECOVERY_{}", key.to_uppercase()), text);
            }
        }
        Ok(env)
    }
}

/// Refuse unsupported output formats instead of silently omitting the requested recovery service.
pub(crate) fn validate_formats(cluster: &ClusterInput, formats: &[OutputType]) -> Result<(), CliError> {
    if !cluster.recovery.is_unset() && formats.iter().any(|format| *format != OutputType::K3s) {
        return Err(CliError::Recovery("recovery currently requires k3s output".into()));
    }
    Ok(())
}

/// Remove only obsolete generated launchers after a recovery-enabled K3s deployment has been written successfully.
pub(crate) fn remove_unsupported_outputs(cluster: &ClusterInput, output_dir: &Path) -> Result<(), CliError> {
    if cluster.recovery.is_unset() {
        return Ok(());
    }
    for format in OutputType::ALL.iter().filter(|format| **format != OutputType::K3s) {
        if let Err(error) = fs::remove_file(output_dir.join(format.output_file_name()))
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(error.into());
        }
    }
    Ok(())
}

/// Bind intervention to declared deployments with stable agent identities, never arbitrary Kubernetes names.
pub(crate) fn validate(cluster: &ClusterInput, runners: &[RunnerInstance]) -> Result<(), CliError> {
    let Recovery::Configured(settings) = &cluster.recovery else {
        return Ok(());
    };
    let _config = settings.config(&format!("et-{}", cluster.cluster_name))?;
    crate::limits::validate_limits("recovery-supervisor", &settings.limits)?;
    if runners.iter().any(|runner| runner.name == "recovery-supervisor") {
        return Err(CliError::Recovery(
            "recovery-supervisor is reserved when recovery is enabled".into(),
        ));
    }
    for workload in &settings.workloads {
        validate_workload(settings, workload, runners)?;
    }
    Ok(())
}

/// Check one declared runner's kind and identity without mixing scenario-wide validation into it.
#[expect(
    clippy::single_call_fn,
    reason = "validate each workload binding as a distinct scenario-generation step"
)]
fn validate_workload(
    settings: &RecoverySettings,
    workload: &Workload,
    runners: &[RunnerInstance],
) -> Result<(), CliError> {
    let runner = runners
        .iter()
        .find(|runner| runner.name == workload.deployment)
        .ok_or_else(|| CliError::Recovery(format!("{} is not a declared runner", workload.deployment)))?;
    if runner.runner != "pyo3" || !runner.schedule.is_unset() {
        return Err(CliError::Recovery(
            "supervised runners must be continuous native-Python deployments".into(),
        ));
    }
    for (name, expected) in bindings(settings, workload) {
        if runner.env.get(&name).is_some_and(|value| *value != expected) {
            return Err(CliError::Recovery(format!(
                "{} conflicts with generated {name}",
                runner.name
            )));
        }
    }
    Ok(())
}

/// Stable runner identity and shared job settings made available to recovery-aware modules.
pub(crate) fn bindings(settings: &RecoverySettings, workload: &Workload) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("PYO3_AGENT_ID".into(), workload.agent_id.clone()),
        ("RECOVERY_JOB_ID".into(), settings.job_id.clone()),
        ("RECOVERY_SUPERVISOR_AGENT_ID".into(), "recovery-supervisor".into()),
    ])
}

/// Include the service image in the ordinary image preparation steps before Kubernetes applies the scenario.
pub(crate) fn image_note(cluster: &ClusterInput) -> String {
    if cluster.recovery.is_unset() {
        return String::default();
    }
    if matches!(cluster.artifact_source, crate::ArtifactSource::Published) {
        return format!(
            "The cluster also pulls `{}/et-recovery-supervisor:latest`.\n\n",
            crate::IMAGE_REGISTRY
        );
    }
    concat!(
        "Build and import the recovery service image from the repository root:\n\n",
        "```bash\n",
        "image=et-recovery-supervisor:latest\n",
        "docker build -t \"$image\" -f services/recovery-supervisor/Dockerfile .\n",
        "docker save \"$image\" | sudo k3s ctr images import -\n",
        "```\n\n",
        "The supervisor starts automatically with scoped ServiceAccount credentials and persistent state.\n",
        "Application modules must implement lifecycle reporting and any required checkpoint restoration.\n\n"
    )
    .into()
}
