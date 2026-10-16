//! Kubernetes recovery deployment: explicit credentials, named-target RBAC and a durable single-owner budget.

use std::collections::BTreeMap;

use et_recovery_supervisor::config::Mode;
use k8s_openapi::api::apps::v1::{Deployment, DeploymentStrategy};
use k8s_openapi::api::core::v1::{
    ConfigMap, ConfigMapVolumeSource, Container, ExecAction, Probe, ServiceAccount, Volume,
};
use k8s_openapi::api::rbac::v1::{PolicyRule, Role, RoleBinding, RoleRef, Subject};

use super::document;
use super::k3s::{
    IMAGE_PULL_POLICY, IMAGE_TAG, claim, deployment, env, hardened_container_context, meta, mount, plain_meta, scratch,
    volume_mount,
};
use crate::placement::hub_site;
use crate::recovery::{Recovery, RecoverySettings};
use crate::{CliError, ClusterInput, RunnerInstance, image_prefix};

const NAME: &str = "recovery-supervisor";
const CONFIG: &str = "recovery-credentials";
const STATE: &str = "recovery-state";

/// Inject only the bindings of allowlisted native-Python workloads.
pub(super) fn configure_runner(cluster: &ClusterInput, runner: &mut RunnerInstance) -> bool {
    if let Recovery::Configured(settings) = &cluster.recovery
        && let Some(workload) = settings
            .workloads
            .iter()
            .find(|workload| workload.deployment == runner.name)
    {
        runner.env.extend(crate::recovery::bindings(settings, workload));
        return true;
    }
    false
}

/// Single-writer replacement for the supervisor budget and supervised application checkpoints.
pub(super) fn recreate(object: &mut Deployment) {
    if let Some(spec) = object.spec.as_mut() {
        spec.strategy = Some(DeploymentStrategy {
            type_: Some("Recreate".into()),
            ..DeploymentStrategy::default()
        });
    }
}

/// Generate nothing unless the scenario explicitly requests recovery.
pub(super) fn documents(cluster: &ClusterInput, namespace: &str) -> Result<Vec<String>, CliError> {
    let Recovery::Configured(settings) = &cluster.recovery else {
        return Ok(Vec::new());
    };
    let account = ServiceAccount {
        metadata: plain_meta(namespace, NAME),
        ..ServiceAccount::default()
    };
    let credentials = ConfigMap {
        metadata: plain_meta(namespace, CONFIG),
        data: Some(BTreeMap::from([("kubeconfig".into(), kubeconfig(namespace)?)])),
        ..ConfigMap::default()
    };
    let mut variables = settings.environment(namespace)?;
    let _previous = variables.insert("RUST_LOG".into(), "info".into());
    let _previous = variables.insert("KUBECONFIG".into(), "/app/credentials/kubeconfig".into());
    let object = supervisor_deployment(cluster, namespace, variables, settings)?;
    Ok(vec![
        document(&account)?,
        document(&role(settings, namespace))?,
        document(&binding(namespace))?,
        document(&credentials)?,
        document(&claim(namespace, STATE))?,
        document(&object)?,
    ])
}

/// Grant access only to named workloads, with mutation reserved for controller-assisted mode.
#[expect(
    clippy::single_call_fn,
    reason = "keep the least-privilege authorization definition explicit"
)]
fn role(settings: &RecoverySettings, namespace: &str) -> Role {
    let verbs = if settings.mode == Mode::ControllerAssisted {
        vec!["get", "patch"]
    } else {
        vec!["get"]
    };
    Role {
        metadata: plain_meta(namespace, NAME),
        rules: Some(vec![PolicyRule {
            api_groups: Some(vec!["apps".into()]),
            resources: Some(vec!["deployments".into()]),
            resource_names: Some(
                settings
                    .workloads
                    .iter()
                    .map(|workload| workload.deployment.clone())
                    .collect(),
            ),
            verbs: verbs.into_iter().map(str::to_owned).collect(),
            ..PolicyRule::default()
        }]),
    }
}

/// Bind the scoped role to this supervisor's service account.
#[expect(
    clippy::single_call_fn,
    reason = "separate role binding from configuration and deployment generation"
)]
fn binding(namespace: &str) -> RoleBinding {
    RoleBinding {
        metadata: plain_meta(namespace, NAME),
        role_ref: RoleRef {
            api_group: "rbac.authorization.k8s.io".into(),
            kind: "Role".into(),
            name: NAME.into(),
        },
        subjects: Some(vec![Subject {
            kind: "ServiceAccount".into(),
            name: NAME.into(),
            namespace: Some(namespace.into()),
            ..Subject::default()
        }]),
    }
}

/// Use the projected, rotating `ServiceAccount` token and cluster CA, never the operator's host kubeconfig.
fn kubeconfig(namespace: &str) -> Result<String, CliError> {
    let service_account = "/var/run/secrets/kubernetes.io/serviceaccount";
    Ok(serde_yaml::to_string(&serde_json::json!({
        "apiVersion": "v1", "kind": "Config", "current-context": "in-cluster",
        "clusters": [{"name": "cluster", "cluster": {
            "server": "https://kubernetes.default.svc", "certificate-authority": format!("{service_account}/ca.crt")
        }}],
        "users": [{"name": "supervisor", "user": {"tokenFile": format!("{service_account}/token")}}],
        "contexts": [{"name": "in-cluster", "context": {
            "cluster": "cluster", "user": "supervisor", "namespace": namespace
        }}]
    }))?)
}

/// Mount persistent state separately from credentials and writable kubectl discovery caches.
fn supervisor_deployment(
    cluster: &ClusterInput,
    namespace: &str,
    variables: BTreeMap<String, String>,
    settings: &RecoverySettings,
) -> Result<Deployment, CliError> {
    let (state_mount, state_volume) = mount(STATE, "/app/state");
    let (runtime_mount, runtime_volume) = scratch("recovery-runtime", "/app/runtime");
    let credentials = credentials_volume();
    let (probe, startup) = health_probes(&settings.config(namespace)?)?;
    let container = Container {
        name: NAME.into(),
        image: Some(format!(
            "{}et-recovery-supervisor:{IMAGE_TAG}",
            image_prefix(cluster.artifact_source)
        )),
        image_pull_policy: Some(IMAGE_PULL_POLICY.into()),
        env: Some(variables.into_iter().map(|(name, value)| env(&name, value)).collect()),
        security_context: Some(hardened_container_context()),
        startup_probe: Some(startup),
        readiness_probe: Some(probe.clone()),
        liveness_probe: Some(probe),
        volume_mounts: Some(vec![
            state_mount,
            runtime_mount,
            volume_mount(CONFIG, "/app/credentials"),
        ]),
        ..Container::default()
    };
    let mut object = deployment(
        namespace,
        NAME,
        container,
        vec![state_volume, runtime_volume, credentials],
        &hub_site(cluster),
        &settings.limits,
    );
    recreate(&mut object);
    if let Some(spec) = object.spec.as_mut().and_then(|spec| spec.template.spec.as_mut()) {
        spec.service_account_name = Some(NAME.into());
        spec.automount_service_account_token = Some(true);
    }
    object.metadata = meta(namespace, NAME);
    Ok(object)
}

/// Project the generated kubeconfig independently of writable state and runtime directories.
#[expect(
    clippy::single_call_fn,
    reason = "the read-only credential volume is a distinct deployment resource"
)]
fn credentials_volume() -> Volume {
    Volume {
        name: CONFIG.into(),
        config_map: Some(ConfigMapVolumeSource {
            name: CONFIG.into(),
            ..ConfigMapVolumeSource::default()
        }),
        ..Volume::default()
    }
}

/// Allow registration and a full bounded cycle before liveness checks can restart a starting supervisor.
#[expect(
    clippy::single_call_fn,
    reason = "probe construction is a distinct deployment-hardening step"
)]
fn health_probes(config: &et_recovery_supervisor::config::Config) -> Result<(Probe, Probe), CliError> {
    let probe = Probe {
        exec: Some(ExecAction {
            command: Some(vec!["et-recovery-healthcheck".into()]),
        }),
        period_seconds: Some(10_i32),
        timeout_seconds: Some(3_i32),
        failure_threshold: Some(3_i32),
        ..Probe::default()
    };
    let startup_seconds = et_recovery_supervisor::health::silence_limit(config)
        .as_secs()
        .saturating_add(30);
    let failure_threshold = match i32::try_from(startup_seconds) {
        Ok(seconds) => seconds,
        Err(error) => {
            return Err(CliError::Recovery(format!(
                "supervisor startup probe duration exceeds Kubernetes limits: {error}"
            )));
        }
    };
    let startup = Probe {
        period_seconds: Some(1_i32),
        failure_threshold: Some(failure_threshold),
        ..probe.clone()
    };
    Ok((probe, startup))
}
