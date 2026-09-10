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
    Capabilities, ConfigMap, ConfigMapEnvSource, Container, ContainerPort, EmptyDirVolumeSource, EnvFromSource, EnvVar,
    ExecAction, HTTPGetAction, Namespace, PersistentVolumeClaim, PersistentVolumeClaimSpec,
    PersistentVolumeClaimVolumeSource, PodSecurityContext, PodSpec, PodTemplateSpec, Probe, ResourceRequirements,
    SeccompProfile, SecretEnvSource, SecurityContext, Service, ServicePort, ServiceSpec, Volume, VolumeMount,
    VolumeResourceRequirements,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use serde::Serialize;

use crate::error::CliError;
use crate::{
    HUB_SERVICE, OutputType, RunnerInstance, cluster_module_names, docker_image_module_paths, hub_service_ws_url,
    module_registry, resolve_cluster_runners,
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

/// Service name of the collector, which is also its Deployment name and the DNS name its in-cluster URL resolves.
///
/// Held as a constant so a URL cannot drift from the `Service` it addresses. Composing the URL from it rather
/// than writing the host into the literal also keeps `link-check` from reading an in-cluster DNS name as an
/// external link it should be able to reach. The hub's equivalent lives beside the URL builder that needs it.
const COLLECTOR_SERVICE: &str = "openobserve";

/// How long a probe waits, and how many misses it tolerates, mirroring `compose.yaml`'s healthchecks.
const PROBE_PERIOD_SECONDS: i32 = 5;
const PROBE_FAILURE_THRESHOLD: i32 = 20;

/// The uid and gid every container runs as, matching the `app` account the repository's images create.
///
/// Above 10000 deliberately: both `services/ws-server/Dockerfile` and `services/ws-web-runner/Dockerfile`
/// declare `USER 10001`, and a uid in the low range is itself a finding since it can collide with a host
/// account. The collector's image ships as uid 0, so this is what moves it off root.
const RUN_AS_ID: i64 = 10001;

/// Directory the hub's writable runtime files are redirected to, so its root filesystem can stay read-only.
///
/// The server generates a self-signed certificate on first start and saves its agent registry on shutdown,
/// both into its working directory. With a read-only root that working directory is not writable, so an
/// `emptyDir` is mounted here and the three paths are pointed at it -- the certificate through the `TLS_*`
/// environment serde reads `TlsConfig` from, the registry through the binary's own `--agent-registry`.
const HUB_RUNTIME_DIR: &str = "/app/runtime";

/// Resource floor and ceiling every container declares.
///
/// Kubernetes schedules a container with no request as best-effort and lets one with no limit consume the
/// node, so both are set. The figures are deliberately generous rather than tuned: a scenario deployment is a
/// demonstration, and a limit that throttles the runners would turn a passing exchange into a flaky one.
const CPU_REQUEST: &str = "100m";
const CPU_LIMIT: &str = "2";
const MEMORY_REQUEST: &str = "128Mi";
const MEMORY_LIMIT: &str = "2Gi";

/// Write `k3s.yaml` for one scenario.
pub fn generate_k3s_deployment(cluster: &ClusterInput, output_dir: &Path) -> Result<(), CliError> {
    let namespace = namespace_name(&cluster.cluster_name);
    let workspace_root = edge_toolkit::config::get_project_root();
    let runners = resolve_cluster_runners(
        &module_registry(&workspace_root, &workspace_root.join("services/ws-server")),
        cluster,
    )?;
    let module_paths = docker_image_module_paths(&cluster_module_names(cluster))?;

    let mut docs = fixed_documents(&namespace, &cluster.cluster_name, &module_paths)?;
    for runner in &runners {
        docs.push(document(&runner_deployment(&namespace, runner))?);
    }

    fs::write(output_dir.join(OutputType::K3s.output_file_name()), docs.join("---\n"))?;

    Ok(())
}

/// Serialise the documents every scenario emits, in apply order.
///
/// Split from the caller so the per-runner documents append to a finished list: each serialisation is a
/// fallible step, and eight of them in one function put it past the cyclomatic-complexity ceiling Codacy
/// enforces without saying anything about how the deployment is shaped.
fn fixed_documents(namespace: &str, cluster_name: &str, module_paths: &[String]) -> Result<Vec<String>, CliError> {
    Ok(vec![
        document(&namespace_object(namespace))?,
        document(&collector_config(namespace))?,
        document(&claim(namespace, "openobserve-data"))?,
        document(&collector_deployment(namespace))?,
        document(&collector_service(namespace))?,
        document(&claim(namespace, "ws-server-storage"))?,
        document(&hub_deployment(namespace, cluster_name, module_paths))?,
        document(&hub_service(namespace))?,
    ])
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

/// The container hardening every container in this file carries.
///
/// Drops every capability, forbids privilege escalation, and takes the root filesystem read-only; anything a
/// container genuinely has to write gets an explicit volume instead. `runAsNonRoot` is belt-and-braces next to
/// the pod-level ids -- it makes the kubelet refuse to start a container that would resolve to uid 0 anyway.
fn hardened_container_context() -> SecurityContext {
    SecurityContext {
        allow_privilege_escalation: Some(false),
        capabilities: Some(Capabilities {
            drop: Some(vec!["ALL".to_string()]),
            ..Capabilities::default()
        }),
        read_only_root_filesystem: Some(true),
        run_as_non_root: Some(true),
        ..SecurityContext::default()
    }
}

/// The pod-level hardening, which is where the ids and the seccomp profile belong.
///
/// `fsGroup` is what makes a mounted claim writable by the non-root user: the volume is chowned to this gid on
/// mount, without which the collector could not write to the data volume it was just moved off root to use.
fn hardened_pod_context() -> PodSecurityContext {
    PodSecurityContext {
        fs_group: Some(RUN_AS_ID),
        run_as_group: Some(RUN_AS_ID),
        run_as_non_root: Some(true),
        run_as_user: Some(RUN_AS_ID),
        seccomp_profile: Some(SeccompProfile {
            type_: "RuntimeDefault".to_string(),
            ..SeccompProfile::default()
        }),
        ..PodSecurityContext::default()
    }
}

/// The request/limit pair every container declares.
fn container_resources() -> ResourceRequirements {
    ResourceRequirements {
        limits: Some(BTreeMap::from([
            ("cpu".to_string(), Quantity(CPU_LIMIT.to_string())),
            ("memory".to_string(), Quantity(MEMORY_LIMIT.to_string())),
        ])),
        requests: Some(BTreeMap::from([
            ("cpu".to_string(), Quantity(CPU_REQUEST.to_string())),
            ("memory".to_string(), Quantity(MEMORY_REQUEST.to_string())),
        ])),
        ..ResourceRequirements::default()
    }
}

/// The container half of a volume pair, which is the same whatever the volume is backed by.
fn volume_mount(name: &str, path: &str) -> VolumeMount {
    VolumeMount {
        mount_path: path.to_string(),
        name: name.to_string(),
        ..VolumeMount::default()
    }
}

/// An in-memory scratch volume, for the paths a read-only root filesystem would otherwise deny.
///
/// `path` is a container mount point in the manifest this generator emits, not a path anything in this process
/// opens, so a caller passing `/tmp` is naming the containerised program's own temp directory rather than
/// creating a world-writable file on the host. `DeepSource`'s `RS-S1003` reads the literal as the latter, which
/// is why every call site passing `/tmp` carries a `skipcq` for that one rule.
fn scratch(name: &str, path: &str) -> (VolumeMount, Volume) {
    let volume = Volume {
        empty_dir: Some(EmptyDirVolumeSource::default()),
        name: name.to_string(),
        ..Volume::default()
    };
    (volume_mount(name, path), volume)
}

/// Mount a claim of the same name at `path`.
fn mount(name: &str, path: &str) -> (VolumeMount, Volume) {
    let volume = Volume {
        name: name.to_string(),
        persistent_volume_claim: Some(PersistentVolumeClaimVolumeSource {
            claim_name: name.to_string(),
            ..PersistentVolumeClaimVolumeSource::default()
        }),
        ..Volume::default()
    };
    (volume_mount(name, path), volume)
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
                    security_context: Some(hardened_pod_context()),
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
    let (data_mount, data_volume) = mount("openobserve-data", "/data");
    // The collector writes transient state outside its data directory, which a read-only root would refuse.
    // skipcq: RS-S1003
    let (tmp_mount, tmp_volume) = scratch("openobserve-tmp", "/tmp");
    let container = Container {
        resources: Some(container_resources()),
        security_context: Some(hardened_container_context()),
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
        volume_mounts: Some(vec![data_mount, tmp_mount]),
        ..Container::default()
    };
    deployment(namespace, COLLECTOR_SERVICE, container, vec![data_volume, tmp_volume])
}

/// Collector service, on the same port `compose.yaml` publishes to loopback.
fn collector_service(namespace: &str) -> Service {
    service(
        namespace,
        COLLECTOR_SERVICE,
        vec![port("http", Services::OtlpCollector.port())],
    )
}

/// The hub's environment, everything it needs that is not the credential the `Secret` carries.
///
/// The paths under the runtime directory are the writable ones: a read-only root filesystem cannot take the
/// self-signed certificate the hub generates on first start, so both halves are redirected to the scratch mount.
fn hub_env(module_paths: &[String]) -> Vec<EnvVar> {
    vec![
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
        env("TLS_CERT_FILE", format!("{HUB_RUNTIME_DIR}/cert.pem")),
        env("TLS_KEY_FILE", format!("{HUB_RUNTIME_DIR}/key.pem")),
    ]
}

/// The hub, running this scenario's image because that is what carries its module set.
fn hub_deployment(namespace: &str, cluster_name: &str, module_paths: &[String]) -> Deployment {
    let insecure = Services::InsecureWebSocketServer.port();
    let (storage_mount, storage_volume) = mount("ws-server-storage", "/app/storage");
    let (runtime_mount, runtime_volume) = scratch("ws-server-runtime", HUB_RUNTIME_DIR);
    let container = Container {
        // `--agent-registry` is a flag rather than an environment variable, so the registry path is the one
        // writable location that has to be redirected through `args` instead of `env`.
        //
        // `command` has to be restated with it. The image declares its binary as `CMD` with no `ENTRYPOINT`,
        // and Kubernetes `args` overrides `CMD` outright rather than appending to it -- so args alone left the
        // kubelet trying to exec the flag as the program:
        //   exec: "--agent-registry": executable file not found in $PATH
        args: Some(vec![
            "--agent-registry".to_string(),
            format!("{HUB_RUNTIME_DIR}/registry.yaml"),
        ]),
        command: Some(vec!["et-ws-server".to_string()]),
        env: Some(hub_env(module_paths)),
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
        resources: Some(container_resources()),
        security_context: Some(hardened_container_context()),
        volume_mounts: Some(vec![storage_mount, runtime_mount]),
        ..Container::default()
    };
    deployment(namespace, HUB_SERVICE, container, vec![storage_volume, runtime_volume])
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
    // A runner fetches its module to a scratch directory and, for the web runner, lets Deno cache there, so a
    // read-only root needs somewhere writable even though nothing is meant to persist.
    // skipcq: RS-S1003
    let (runtime_mount, runtime_volume) = scratch("runner-tmp", "/tmp");
    let container = Container {
        env: Some(vec![
            env("RUNNER_MODULE", runner.module.clone()),
            env("WS_SERVER_URL", hub_service_ws_url()),
        ]),
        image: Some(format!("et-ws-{}-runner:{IMAGE_TAG}", runner.runner)),
        image_pull_policy: Some(IMAGE_PULL_POLICY.to_string()),
        name: runner.name.clone(),
        resources: Some(container_resources()),
        security_context: Some(hardened_container_context()),
        volume_mounts: Some(vec![runtime_mount]),
        ..Container::default()
    };
    deployment(namespace, &runner.name, container, vec![runtime_volume])
}
