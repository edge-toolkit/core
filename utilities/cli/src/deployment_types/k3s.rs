//! k3s deployment: one multi-document `k3s.yaml` of plain Kubernetes manifests per scenario.
//!
//! Plain manifests rather than a Helm chart or a kustomize overlay, because the scenario input already fixes
//! every value a chart would parameterise -- the cluster name, the module set, the runner list -- so a chart
//! would add a templating layer over inputs that are known at generation time. It also keeps the output a
//! single file the drift check can diff, the same way `mise.toml` and `compose.yaml` are.
//!
//! The objects are `k8s-openapi`'s, not hand-written structs, so every field name and nesting is the API's own
//! rather than this file's guess at it -- a mistyped `imagePullPolicy` is a compile error instead of a manifest
//! the cluster rejects.
//!
//! The translation from `compose.yaml` is not line-for-line, because two of compose's mechanisms have no
//! Kubernetes equivalent and their replacements are better:
//!
//! * compose runs the hub and every runner on `network_mode: host`, so each addresses the others as
//!   `localhost`. Here each component gets a `Service`, and the runners reach the hub by its in-namespace DNS
//!   name. Nothing binds a host port, so two scenarios can run side by side.
//! * compose expresses start-up order with `depends_on: condition: service_healthy`. Kubernetes has no such
//!   edge. The hub and collector carry readiness probes, and a runner that starts early exits and is restarted
//!   until the hub answers -- `CrashLoopBackOff` on the way up is expected here, not a fault.
//!
//! Nothing in this file carries the scenario's credential. The deployment reads it from a `Secret` the operator
//! creates from the generated env file, so the manifests stay committable.

use std::collections::BTreeMap;
use std::path::Path;

use edge_toolkit::input::ClusterInput;
use edge_toolkit::ports::Services;
use fs_err as fs;
use k8s_openapi::api::apps::v1::{Deployment, DeploymentSpec};
use k8s_openapi::api::core::v1::{
    ConfigMap, ConfigMapEnvSource, Container, ContainerPort, EnvFromSource, EnvVar, ExecAction, HTTPGetAction,
    Namespace, PersistentVolumeClaim, PersistentVolumeClaimSpec, PersistentVolumeClaimVolumeSource, PodSpec,
    PodTemplateSpec, Probe, SecretEnvSource, Service, ServicePort, ServiceSpec, Volume, VolumeMount,
    VolumeResourceRequirements,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use serde::Serialize;

use crate::error::CliError;
use crate::{
    OutputType, RunnerInstance, cluster_module_names, docker_image_module_paths, module_registry,
    resolve_cluster_runners,
};

/// Pull policy that lets an image built from this repository and imported into the node satisfy a manifest.
///
/// `IfNotPresent` rather than the `Always` a `:latest` tag defaults to: the scenario images never reach a
/// registry, so an `Always` pull would send k3s looking for them in one that does not have them.
const IMAGE_PULL_POLICY: &str = "IfNotPresent";

/// Tag the generated manifests reference, matching what the README's build-and-import step produces.
const IMAGE_TAG: &str = "latest";

/// Collector image, matching the one `compose.yaml` runs.
const OPENOBSERVE_IMAGE: &str = "openobserve/openobserve:v0.91.5";

/// Service names, which are also the Deployment names and the DNS names the in-cluster URLs resolve.
///
/// Held as constants so a URL cannot drift from the `Service` it addresses. Composing the URLs from them
/// rather than writing the host into the literal also keeps `link-check` from reading an in-cluster DNS name
/// as an external link it should be able to reach.
const COLLECTOR_SERVICE: &str = "openobserve";
const HUB_SERVICE: &str = "ws-server";

/// How long a probe waits, and how many misses it tolerates, mirroring `compose.yaml`'s healthchecks.
const PROBE_PERIOD_SECONDS: i32 = 5;
const PROBE_FAILURE_THRESHOLD: i32 = 20;

/// Write `k3s.yaml` for one scenario.
pub fn generate_k3s_deployment(cluster: &ClusterInput, output_dir: &Path) -> Result<(), CliError> {
    let namespace = namespace_name(&cluster.cluster_name);
    let workspace_root = edge_toolkit::config::get_project_root();
    let runners = resolve_cluster_runners(
        &module_registry(&workspace_root, &workspace_root.join("services/ws-server")),
        cluster,
    )?;
    let module_paths = docker_image_module_paths(&cluster_module_names(cluster))?;

    let mut docs = vec![
        document(&namespace_object(&namespace))?,
        document(&collector_config(&namespace))?,
        document(&claim(&namespace, "openobserve-data"))?,
        document(&collector_deployment(&namespace))?,
        document(&collector_service(&namespace))?,
        document(&claim(&namespace, "ws-server-storage"))?,
        document(&hub_deployment(&namespace, &cluster.cluster_name, &module_paths))?,
        document(&hub_service(&namespace))?,
    ];
    for runner in &runners {
        docs.push(document(&runner_deployment(&namespace, runner))?);
    }

    fs::write(output_dir.join(OutputType::K3s.output_file_name()), docs.join("---\n"))?;

    Ok(())
}

/// Serialise one object, in the YAML style `dprint-check` expects of a committed file.
#[expect(
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    reason = "pretty_yaml only fails on malformed YAML and serde output is always well-formed"
)]
fn document<T>(object: &T) -> Result<String, CliError>
where
    T: Serialize,
{
    let yaml = serde_yaml::to_string(object)?;
    Ok(pretty_yaml::format_text(&yaml, &pretty_yaml::config::FormatOptions::default()).unwrap())
}

/// Namespace a scenario's objects live in, so two scenarios can be applied to one cluster at once.
fn namespace_name(cluster_name: &str) -> String {
    format!("et-{cluster_name}")
}

/// Name of the `Secret` the deployment expects the operator to have created from the generated env file.
fn secret_name(namespace: &str) -> String {
    format!("{namespace}-secrets")
}

/// The `app` label every selector in this file matches on.
fn labels(name: &str) -> BTreeMap<String, String> {
    BTreeMap::from([("app".to_string(), name.to_string())])
}

/// Metadata for a namespaced object, labelled so its Service can select it.
fn meta(namespace: &str, name: &str) -> ObjectMeta {
    ObjectMeta {
        labels: Some(labels(name)),
        name: Some(name.to_string()),
        namespace: Some(namespace.to_string()),
        ..ObjectMeta::default()
    }
}

/// Metadata for an object that holds no pods, so nothing ever selects it by label.
fn plain_meta(namespace: &str, name: &str) -> ObjectMeta {
    ObjectMeta {
        name: Some(name.to_string()),
        namespace: Some(namespace.to_string()),
        ..ObjectMeta::default()
    }
}

/// The namespace object itself, which carries no namespace of its own.
fn namespace_object(namespace: &str) -> Namespace {
    Namespace {
        metadata: ObjectMeta {
            name: Some(namespace.to_string()),
            ..ObjectMeta::default()
        },
        ..Namespace::default()
    }
}

/// The collector's non-secret settings, which are the committed half of what `config/o2.env` holds.
///
/// Its root password is the other half and is deliberately absent: that value reaches the pod from the
/// operator-created `Secret`, so nothing here has to be redacted before the file is committed.
fn collector_config(namespace: &str) -> ConfigMap {
    ConfigMap {
        data: Some(BTreeMap::from([
            ("RUST_LOG".to_string(), "warn".to_string()),
            ("ZO_DATA_DIR".to_string(), "/data".to_string()),
            ("ZO_ROOT_USER_EMAIL".to_string(), "root@example.com".to_string()),
        ])),
        metadata: plain_meta(namespace, "openobserve-config"),
        ..ConfigMap::default()
    }
}

/// A one-gigabyte `ReadWriteOnce` claim, standing in for the named volumes `compose.yaml` declares.
fn claim(namespace: &str, name: &str) -> PersistentVolumeClaim {
    PersistentVolumeClaim {
        metadata: plain_meta(namespace, name),
        spec: Some(PersistentVolumeClaimSpec {
            access_modes: Some(vec!["ReadWriteOnce".to_string()]),
            resources: Some(VolumeResourceRequirements {
                requests: Some(BTreeMap::from([("storage".to_string(), Quantity("1Gi".to_string()))])),
                ..VolumeResourceRequirements::default()
            }),
            ..PersistentVolumeClaimSpec::default()
        }),
        ..PersistentVolumeClaim::default()
    }
}

/// Read a whole `ConfigMap` into the container environment.
fn from_config_map(name: &str) -> EnvFromSource {
    EnvFromSource {
        config_map_ref: Some(ConfigMapEnvSource {
            name: name.to_string(),
            ..ConfigMapEnvSource::default()
        }),
        ..EnvFromSource::default()
    }
}

/// Read the operator-created `Secret` into the container environment.
fn from_secret(namespace: &str) -> EnvFromSource {
    EnvFromSource {
        secret_ref: Some(SecretEnvSource {
            name: secret_name(namespace),
            ..SecretEnvSource::default()
        }),
        ..EnvFromSource::default()
    }
}

/// One literal environment entry.
fn env(name: &str, value: String) -> EnvVar {
    EnvVar {
        name: name.to_string(),
        value: Some(value),
        ..EnvVar::default()
    }
}

/// A readiness probe on the shared cadence, differing only in how it asks the container whether it is up.
fn probe(exec: Option<ExecAction>, http_get: Option<HTTPGetAction>) -> Probe {
    Probe {
        exec,
        failure_threshold: Some(PROBE_FAILURE_THRESHOLD),
        http_get,
        period_seconds: Some(PROBE_PERIOD_SECONDS),
        ..Probe::default()
    }
}

/// Mount a claim of the same name at `path`.
fn mount(name: &str, path: &str) -> (VolumeMount, Volume) {
    let volume_mount = VolumeMount {
        mount_path: path.to_string(),
        name: name.to_string(),
        ..VolumeMount::default()
    };
    let volume = Volume {
        name: name.to_string(),
        persistent_volume_claim: Some(PersistentVolumeClaimVolumeSource {
            claim_name: name.to_string(),
            ..PersistentVolumeClaimVolumeSource::default()
        }),
        ..Volume::default()
    };
    (volume_mount, volume)
}

/// Wrap a container in the single-replica Deployment every component here uses.
fn deployment(namespace: &str, name: &str, container: Container, volumes: Vec<Volume>) -> Deployment {
    Deployment {
        metadata: meta(namespace, name),
        spec: Some(DeploymentSpec {
            replicas: Some(1),
            selector: LabelSelector {
                match_labels: Some(labels(name)),
                ..LabelSelector::default()
            },
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(labels(name)),
                    ..ObjectMeta::default()
                }),
                spec: Some(PodSpec {
                    containers: vec![container],
                    volumes: (!volumes.is_empty()).then_some(volumes),
                    ..PodSpec::default()
                }),
            },
            ..DeploymentSpec::default()
        }),
        ..Deployment::default()
    }
}

/// A `ClusterIP` service, which is what the runners resolve instead of compose's shared host network.
fn service(namespace: &str, name: &str, ports: Vec<ServicePort>) -> Service {
    Service {
        metadata: meta(namespace, name),
        spec: Some(ServiceSpec {
            ports: Some(ports),
            selector: Some(labels(name)),
            ..ServiceSpec::default()
        }),
        ..Service::default()
    }
}

/// One named port, published on the same number the container listens on.
fn port(name: &str, number: u16) -> ServicePort {
    ServicePort {
        name: Some(name.to_string()),
        port: i32::from(number),
        target_port: Some(IntOrString::Int(i32::from(number))),
        ..ServicePort::default()
    }
}

/// The collector, reading its password from the operator-created secret and everything else from the `ConfigMap`.
fn collector_deployment(namespace: &str) -> Deployment {
    let (volume_mount, volume) = mount("openobserve-data", "/data");
    let container = Container {
        env_from: Some(vec![from_config_map("openobserve-config"), from_secret(namespace)]),
        image: Some(OPENOBSERVE_IMAGE.to_string()),
        image_pull_policy: Some(IMAGE_PULL_POLICY.to_string()),
        name: COLLECTOR_SERVICE.to_string(),
        ports: Some(vec![ContainerPort {
            container_port: i32::from(Services::OtlpCollector.port()),
            ..ContainerPort::default()
        }]),
        readiness_probe: Some(probe(
            Some(ExecAction {
                command: Some(vec![
                    "/openobserve".to_string(),
                    "node".to_string(),
                    "status".to_string(),
                ]),
            }),
            None,
        )),
        volume_mounts: Some(vec![volume_mount]),
        ..Container::default()
    };
    deployment(namespace, COLLECTOR_SERVICE, container, vec![volume])
}

/// Collector service, on the same port `compose.yaml` publishes to loopback.
fn collector_service(namespace: &str) -> Service {
    service(
        namespace,
        COLLECTOR_SERVICE,
        vec![port("http", Services::OtlpCollector.port())],
    )
}

/// The hub, running this scenario's image because that is what carries its module set.
fn hub_deployment(namespace: &str, cluster_name: &str, module_paths: &[String]) -> Deployment {
    let insecure = Services::InsecureWebSocketServer.port();
    let (volume_mount, volume) = mount("ws-server-storage", "/app/storage");
    let container = Container {
        env: Some(vec![
            env("MODULES_PATHS", module_paths.join(",")),
            env(
                "OTLP_COLLECTOR_URL",
                format!(
                    "http://{}:{}/api/default/v1",
                    COLLECTOR_SERVICE,
                    Services::OtlpCollector.port()
                ),
            ),
            env("STORAGE_URL", "file:///app/storage".to_string()),
        ]),
        env_from: Some(vec![from_secret(namespace)]),
        image: Some(format!("et-ws-server-{cluster_name}:{IMAGE_TAG}")),
        image_pull_policy: Some(IMAGE_PULL_POLICY.to_string()),
        name: HUB_SERVICE.to_string(),
        ports: Some(vec![
            ContainerPort {
                container_port: i32::from(insecure),
                ..ContainerPort::default()
            },
            ContainerPort {
                container_port: i32::from(Services::SecureWebSocketServer.port()),
                ..ContainerPort::default()
            },
        ]),
        readiness_probe: Some(probe(
            None,
            Some(HTTPGetAction {
                path: Some("/health".to_string()),
                port: IntOrString::Int(i32::from(insecure)),
                ..HTTPGetAction::default()
            }),
        )),
        volume_mounts: Some(vec![volume_mount]),
        ..Container::default()
    };
    deployment(namespace, HUB_SERVICE, container, vec![volume])
}

/// Hub service, carrying both the insecure and secure ports the server listens on.
fn hub_service(namespace: &str) -> Service {
    service(
        namespace,
        HUB_SERVICE,
        vec![
            port("ws", Services::InsecureWebSocketServer.port()),
            port("wss", Services::SecureWebSocketServer.port()),
        ],
    )
}

/// One deployment per runner, each hosting a single module exactly as the other two formats arrange it.
fn runner_deployment(namespace: &str, runner: &RunnerInstance) -> Deployment {
    let container = Container {
        env: Some(vec![
            env("RUNNER_MODULE", runner.module.clone()),
            env(
                "WS_SERVER_URL",
                format!("ws://{}:{}/ws", HUB_SERVICE, Services::InsecureWebSocketServer.port()),
            ),
        ]),
        image: Some(format!("et-ws-{}-runner:{IMAGE_TAG}", runner.runner)),
        image_pull_policy: Some(IMAGE_PULL_POLICY.to_string()),
        name: runner.name.clone(),
        ..Container::default()
    };
    deployment(namespace, &runner.name, container, Vec::new())
}
