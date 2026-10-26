//! Namespace-scoped Kubernetes operations; targets come only from validated operator configuration.

use std::time::Duration;

use serde::Deserialize;
use tokio::process::Command;

use crate::RecoveryError;
use crate::config::{Config, Workload};
use crate::policy::Health;

/// Deployment-specific operations consumed by the supervisor, which an in-memory backend can stand in for.
pub trait Backend: Send {
    /// Observe readiness; failures must remain errors, not be interpreted as application failure.
    fn health(&mut self, workload: &Workload) -> impl Future<Output = Result<Health, RecoveryError>> + Send;
    /// Request recovery of an allowlisted workload; success means accepted, not recovered.
    fn restart(&mut self, workload: &Workload) -> impl Future<Output = Result<(), RecoveryError>> + Send;
}

/// Kubernetes backend using the installed kubectl credentials and an explicit context and namespace.
#[derive(Debug)]
pub struct Kubernetes {
    config: Config,
}

impl Kubernetes {
    /// Validate targets before any external command is possible.
    pub fn new(config: Config) -> Result<Self, RecoveryError> {
        config.validate()?;
        Ok(Self { config })
    }

    /// Execute kubectl directly, without shell interpolation, bounding both the API request and child lifetime.
    pub async fn execute(&self, workload: &Workload, arguments: &[&str]) -> Result<Vec<u8>, RecoveryError> {
        if !self
            .config
            .workloads
            .iter()
            .any(|item| item.deployment == workload.deployment)
        {
            return Err(RecoveryError::Invalid("Kubernetes target is not allowlisted".into()));
        }
        let target = format!("deployment/{}", workload.deployment);
        let mut command = Command::new("kubectl");
        let _command = command
            .args([
                "--context",
                &self.config.context,
                "--namespace",
                &self.config.namespace,
                "--request-timeout=5s",
            ])
            .args(arguments)
            .arg(target);
        let output =
            crate::command::output(&mut command, Duration::from_secs(self.config.command_timeout_seconds)).await?;
        if !output.status.success() {
            return Err(RecoveryError::Invalid(format!(
                "kubectl {arguments:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(output.stdout)
    }
}

#[derive(Deserialize)]
struct Deployment {
    metadata: Metadata,
    spec: Spec,
    #[serde(default)]
    status: DeploymentStatus,
}

#[derive(Deserialize)]
struct Metadata {
    generation: u64,
}

#[derive(Deserialize)]
struct Spec {
    replicas: u32,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeploymentStatus {
    #[serde(default)]
    observed_generation: u64,
    #[serde(default)]
    available_replicas: u32,
    #[serde(default)]
    updated_replicas: u32,
    #[serde(default)]
    replicas: u32,
    #[serde(default)]
    unavailable_replicas: u32,
}

/// Decode readiness without assuming that an HTTP-available process has resumed application work.
pub fn deployment_health(bytes: &[u8]) -> Result<Health, RecoveryError> {
    let deployment: Deployment = serde_json::from_slice(bytes)?;
    if deployment.spec.replicas != 1 {
        return Err(RecoveryError::Invalid(
            "supervised deployments must have exactly one desired replica".into(),
        ));
    }
    let status = deployment.status;
    Ok(
        if status.observed_generation >= deployment.metadata.generation
            && status.available_replicas == 1
            && status.updated_replicas == 1
            && status.replicas == 1
            && status.unavailable_replicas == 0
        {
            Health::Ready
        } else {
            Health::Unready
        },
    )
}

impl Backend for Kubernetes {
    /// Query and decode readiness for one allowlisted deployment.
    async fn health(&mut self, workload: &Workload) -> Result<Health, RecoveryError> {
        deployment_health(&self.execute(workload, &["get", "-o", "json"]).await?)
    }

    /// Request a bounded rollout restart for one allowlisted deployment.
    async fn restart(&mut self, workload: &Workload) -> Result<(), RecoveryError> {
        let _output = self.execute(workload, &["rollout", "restart"]).await?;
        Ok(())
    }
}
