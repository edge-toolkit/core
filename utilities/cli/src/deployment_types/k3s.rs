//! k3s deployment: one multi-document `k3s.yaml` of plain Kubernetes manifests per scenario.
//!
//! Plain manifests rather than a Helm chart or a kustomize overlay, because the scenario input already fixes every
//! value a chart would parameterise -- the cluster name, the module set, the runner list -- so a chart would add a
//! templating layer over inputs that are known at generation time. It also keeps the output a single file the drift
//! check can diff, the same way `mise.toml` and `compose.yaml` are.
//!
//! The objects are `k8s-openapi`'s, not hand-written structs, so every field name and nesting is the API's own rather
//! than this file's guess at it -- a mistyped `imagePullPolicy` is a compile error instead of a manifest the cluster
//! rejects.
//!
//! The translation from `compose.yaml` is not line-for-line, because two of compose's mechanisms have no Kubernetes
//! equivalent and their replacements are better:
//!
//! * compose runs the hub and every runner on `network_mode: host`, so each addresses the others as
//!   `localhost`. Here each component gets a `Service`, and the runners reach the hub by its in-namespace DNS
//!   name. Nothing binds a host port, so two scenarios can run side by side.
//! * compose expresses start-up order with `depends_on: condition: service_healthy`. Kubernetes has no such
//!   edge. The hub and collector carry readiness probes, and a runner that starts early exits and is restarted
//!   until the hub answers -- `CrashLoopBackOff` on the way up is expected here, not a fault.
//!
//! Nothing in this file carries the scenario's credential. The deployment reads it from a `Secret` the operator creates
//! from the generated env file, so the manifests stay committable.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use edge_toolkit::ports::Services;
use fs_err as fs;
use k8s_openapi::api::apps::v1::{DaemonSet, DaemonSetSpec, Deployment, DeploymentSpec};
use k8s_openapi::api::batch::v1::{CronJob, CronJobSpec, JobSpec, JobTemplateSpec};
use k8s_openapi::api::core::v1::{
    Capabilities, ConfigMap, ConfigMapEnvSource, ConfigMapVolumeSource, Container, ContainerPort, EmptyDirVolumeSource,
    EnvFromSource, EnvVar, ExecAction, HTTPGetAction, HostPathVolumeSource, Namespace, PersistentVolumeClaim,
    PersistentVolumeClaimSpec, PersistentVolumeClaimVolumeSource, PodSecurityContext, PodSpec, PodTemplateSpec, Probe,
    ResourceRequirements, SeccompProfile, SecretEnvSource, SecurityContext, Service, ServicePort, ServiceSpec, Volume,
    VolumeMount, VolumeResourceRequirements,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;

use super::document;
use crate::error::CliError;
use crate::input::{ArtifactSource, ClusterInput};
use crate::limits::{CpuCap, Limits, MAX_CPU_SHARES, ResolvedLimits, k8s_bytes, k8s_cpus};
use crate::placement::{MACHINE_LABEL, Site, hub_site};
use crate::schedule::Schedule;
use crate::{
    COLLECTOR_SETTINGS, COLLECTOR_USERNAME, HUB_SERVICE, OutputType, RunnerInstance, cluster_module_names,
    docker_image_module_paths, hub_service_ws_url, image_prefix, module_registry, resolve_cluster_runners,
};

/// Pull policy that lets an image built from this repository and imported into the node satisfy a manifest.
///
/// `IfNotPresent` rather than the `Always` a `:latest` tag defaults to, and it is the right answer whichever
/// `artifact_source` a scenario asks for. A locally built image never reaches a registry at all, so an `Always`
/// pull would send k3s looking for it in one that does not have it. A published image is there to be found, and
/// `IfNotPresent` is what stops every restart re-pulling a tag the node already holds. It also lets a published-image
/// scenario be tested against a local build of the same tag, which is how CI checks a change to an image before it
/// is published.
const IMAGE_PULL_POLICY: &str = "IfNotPresent";

/// Tag the generated manifests reference, matching what the README's build-and-import step produces.
const IMAGE_TAG: &str = "latest";

/// Collector image, matching the one `compose.yaml` runs.
const OPENOBSERVE_IMAGE: &str = "openobserve/openobserve:v0.91.5";

/// Service name of the collector, which is also its Deployment name and the DNS name its in-cluster URL resolves.
///
/// Held as a constant so a URL cannot drift from the `Service` it addresses. Composing the URL from it rather than
/// writing the host into the literal also keeps link checking from reading an in-cluster DNS name as an external link
/// it should be able to reach. The hub's equivalent lives beside the URL builder that needs it.
const COLLECTOR_SERVICE: &str = "openobserve";

/// How long a probe waits, and how many misses it tolerates, mirroring `compose.yaml`'s healthchecks.
const PROBE_PERIOD_SECONDS: i32 = 5;
const PROBE_FAILURE_THRESHOLD: i32 = 20;

/// The uid and gid every container runs as, matching the `app` account the repository's images create.
///
/// Above 10000 deliberately: both `services/ws-server/Dockerfile` and `services/ws-web-runner/Dockerfile` declare `USER
/// 10001`, and a uid in the low range is itself a finding since it can collide with a host account. The collector's
/// image ships as uid 0, so this is what moves it off root.
const RUN_AS_ID: i64 = 10001;

/// Directory the hub's writable runtime files are redirected to, so its root filesystem can stay read-only.
///
/// The server generates a self-signed certificate on first start and saves its agent registry on shutdown, both
/// into its working directory. With a read-only root that working directory is not writable, so an `emptyDir` is
/// mounted here and the three paths are pointed at it -- the certificate through the `TLS_*` environment serde reads
/// `TlsConfig` from, the registry through the binary's own `--agent-registry`.
const HUB_RUNTIME_DIR: &str = "/app/runtime";

/// The volume a container's sized `/dev/shm` is mounted from.
const SHM_VOLUME: &str = "shm";

/// Where the container runtime plugins run: they serve every pod on a node, whichever scenario it belongs to.
///
/// Two scenarios applied to one cluster both declare the plugins, and declaring them in one namespace of their own
/// under fixed names makes the second apply a no-op rather than a second copy of each plugin on every node. Not `kube-
/// system`, which is the cluster's own and has no business holding workloads this project brings.
pub(crate) const PLUGIN_NAMESPACE: &str = "et-nri-plugins";

/// containerd's own NRI plugin that sets a container's rlimits from the annotation [`ULIMITS_ANNOTATION`] names.
static ULIMIT_ADJUSTER: RuntimePlugin = RuntimePlugin {
    args: &["-idx", "10"],
    command: &[],
    config: None,
    image: "ghcr.io/containerd/nri/plugins/ulimit-adjuster:v0.10.0",
    name: "ulimit-adjuster",
    workload: "nri-plugin-ulimit-adjuster",
};

/// The nri-plugins project's plugin that writes a cgroup v2 file from each `<file>.memory-qos.nri.io/<container>`.
///
/// Despite its name it writes any file its configuration lists, which [`MEMORY_QOS_FILES`] makes the set of cgroup
/// files the Pod API has no field for.
static MEMORY_QOS: RuntimePlugin = RuntimePlugin {
    args: &["--idx", "90", "--config", "/etc/nri/memory-qos/config.yaml"],
    command: &["nri-memory-qos"],
    config: Some(("nri-memory-qos-config", "/etc/nri/memory-qos")),
    image: "ghcr.io/containers/nri-plugins/nri-memory-qos:v0.14.0",
    name: "nri-memory-qos",
    workload: "nri-memory-qos",
};

/// The cgroup v2 files memory-qos is allowed to write, one per scenario limit the Pod API cannot express.
const MEMORY_QOS_FILES: [&str; 7] = [
    "cpu.max",
    "cpu.weight",
    "cpuset.cpus",
    "io.bfq.weight",
    "memory.low",
    "memory.swap.max",
    "pids.max",
];

/// The annotation, suffixed with a container name, that ulimit-adjuster reads that container's rlimits from.
const ULIMITS_ANNOTATION: &str = "ulimits.noderesource.dev/container";

/// The annotation, suffixed with a container name, that has the runtime refuse a container its plugins did not adjust.
///
/// NRI's default validator, enabled by the node preparation the README describes, rejects the container when any plugin
/// named here has not processed it. That is what turns a node without the plugins into a pod that does not start,
/// rather than one that starts without its limits.
const REQUIRED_PLUGINS_ANNOTATION: &str = "required-plugins.noderesource.dev/container";

/// The file each machine's containerd configuration is extended from, written beside `k3s.yaml` for any plugin.
pub(crate) const CONTAINERD_TEMPLATE_FILE: &str = "containerd-config-v3.toml.tmpl";

/// Where k3s reads a containerd configuration template from, on every machine of the cluster.
const CONTAINERD_TEMPLATE_PATH: &str = "/var/lib/rancher/k3s/agent/etc/containerd/config-v3.toml.tmpl";

/// k3s's own containerd configuration with NRI's default validator turned on.
///
/// The validator is what reads the required-plugins annotations; k3s ships it off, so without this a container whose
/// plugin is missing would start without its limits instead of being refused. `base` is the configuration k3s would
/// otherwise write, which carries no NRI table of its own, so the section below adds to it rather than redefining one.
const CONTAINERD_TEMPLATE: &str = concat!(
    "{{ template \"base\" . }}\n",
    "\n",
    "[plugins.\"io.containerd.nri.v1.nri\".default_validator]\n",
    "  enable = true\n"
);

/// The socket containerd's NRI listens on, which each plugin connects to.
const NRI_SOCKET: &str = "/var/run/nri/nri.sock";

/// One upstream container runtime plugin, run unmodified as a `DaemonSet`.
struct RuntimePlugin {
    /// The arguments the plugin starts with; `-idx` orders it among the node's plugins, as its upstream manifest does.
    args: &'static [&'static str],
    /// The program to run, or empty for the image's own entrypoint.
    command: &'static [&'static str],
    /// The `ConfigMap` holding the plugin's configuration and where it is mounted; absent, the plugin reads none.
    config: Option<(&'static str, &'static str)>,
    /// The pinned image the plugin is published as.
    image: &'static str,
    /// The name the plugin registers with the runtime, which is what a required-plugins annotation names.
    name: &'static str,
    /// The `DaemonSet` and the label its pods carry.
    workload: &'static str,
}

/// Write `k3s.yaml` for one scenario.
pub fn generate_k3s_deployment(cluster: &ClusterInput, output_dir: &Path) -> Result<(), CliError> {
    let namespace = namespace_name(&cluster.cluster_name);
    let workspace_root = edge_toolkit::config::get_project_root();
    let runners = resolve_cluster_runners(
        &module_registry(
            &workspace_root,
            &workspace_root.join("services/ws-server"),
            &cluster.module_paths,
        ),
        cluster,
    )?;
    let serves_a_page = super::serves_a_page(cluster);
    let module_paths = docker_image_module_paths(&cluster_module_names(cluster), serves_a_page, &cluster.module_paths)?;
    // Empty for a headless cluster, which is not given the page: the hub rejects a root it cannot find among the
    // modules it was given, and serves nothing at `/` when named none.
    let root_module = if serves_a_page {
        super::hub_root_module(&workspace_root.join("services/ws-server"))
    } else {
        String::default()
    };

    let mut docs = fixed_documents(&namespace, cluster, &module_paths, &root_module, &hub_site(cluster))?;
    for runner in &runners {
        docs.push(runner_document(&namespace, runner, cluster.artifact_source)?);
    }
    let plugins = cluster_plugins(cluster);
    docs.extend(runtime_plugin_documents(&plugins)?);

    fs::write(output_dir.join(OutputType::K3s.output_file_name()), docs.join("---\n"))?;
    if !plugins.is_empty() {
        fs::write(output_dir.join(CONTAINERD_TEMPLATE_FILE), CONTAINERD_TEMPLATE)?;
    }

    Ok(())
}

/// Serialise the documents every scenario emits, in apply order.
///
/// Split from the caller so the per-runner documents append to a finished list: each serialisation is a fallible step,
/// and eight of them in one function put it past the repo's cyclomatic-complexity ceiling without saying anything about
/// how the deployment is shaped.
///
/// `site` places both the collector and the hub. Each mounts a claim, and k3s provisions a claim on the node that first
/// mounts it, so the two stay on the server machine rather than going wherever the scheduler finds room.
fn fixed_documents(
    namespace: &str,
    cluster: &ClusterInput,
    module_paths: &[String],
    root_module: &str,
    site: &Site,
) -> Result<Vec<String>, CliError> {
    Ok(vec![
        document(&namespace_object(namespace))?,
        document(&collector_config(namespace))?,
        document(&claim(namespace, "openobserve-data"))?,
        document(&collector_deployment(
            namespace,
            site,
            &cluster.services.collector.limits,
        ))?,
        document(&collector_service(namespace))?,
        document(&claim(namespace, "ws-server-storage"))?,
        document(&hub_deployment(
            namespace,
            &cluster.cluster_name,
            module_paths,
            root_module,
            site,
            &cluster.services.hub.limits,
        ))?,
        document(&hub_service(namespace))?,
    ])
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
/// Its root password is the other half and is deliberately absent: that value reaches the pod from the operator-created
/// `Secret`, so nothing here has to be redacted before the file is committed.
fn collector_config(namespace: &str) -> ConfigMap {
    let mut data: BTreeMap<String, String> = COLLECTOR_SETTINGS
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect();
    // The claim this deployment mounts is what makes a data directory worth naming, so it is set here rather than in
    // the shared list.
    let _previous: Option<String> = data.insert("ZO_DATA_DIR".to_string(), "/data".to_string());
    ConfigMap {
        data: Some(data),
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
/// Drops every capability, forbids privilege escalation, and takes the root filesystem read-only; anything a container
/// genuinely has to write gets an explicit volume instead. `runAsNonRoot` is belt-and-braces next to the pod-level ids
/// -- it makes the kubelet refuse to start a container that would resolve to uid 0 anyway.
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
/// `fsGroup` is what makes a mounted claim writable by the non-root user: the volume is chowned to this gid on mount,
/// without which the collector could not write to the data volume it was just moved off root to use.
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

/// The request/limit pair a container declares, from the limits both formats resolve the same way.
///
/// A quota cap is stated here in whole millicores, rounded up, which is as close as the Pod API gets; the exact quota
/// and period reach the cgroup through [`runtime_annotations`].
fn container_resources(limits: &ResolvedLimits) -> ResourceRequirements {
    let pair = |cpu, memory| {
        BTreeMap::from([
            ("cpu".to_string(), Quantity(k8s_cpus(cpu))),
            ("memory".to_string(), Quantity(k8s_bytes(memory))),
        ])
    };
    ResourceRequirements {
        limits: Some(pair(limits.cpu_cap.millicores(), limits.memory)),
        requests: Some(pair(limits.cpu_request, limits.memory_request)),
        ..ResourceRequirements::default()
    }
}

/// The cgroup v2 `cpu.weight` the kernel gets for `shares`, by the conversion runc applies to Docker's `--cpu-shares`.
///
/// Reproduced rather than approximated, so a container given its weight this way under k3s ends up with the value
/// compose's `cpu_shares` gives the same container: runc maps 2 to 1, 1024 to 100 and 262144 to 10000 along a quadratic
/// in log space, and rounds up.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::float_arithmetic,
    reason = "runc's conversion is defined in floating point; its result is in 1..=10000, so the cast back is exact"
)]
fn cpu_weight(shares: u64) -> u64 {
    if shares <= 2 {
        return 1;
    }
    if shares >= MAX_CPU_SHARES {
        return 10_000;
    }
    let log = f64::from(u32::try_from(shares).unwrap_or(u32::MAX)).log2();
    let exponent = log.mul_add(log, 125.0_f64 * log) / 612.0_f64 - 7.0_f64 / 34.0_f64;
    10.0_f64.powf(exponent).ceil() as u64
}

/// The cgroup files and rlimits a container's limits set beyond what the Pod API carries, as plugin annotations.
///
/// Empty when the Pod API carries every limit the process sets, which is every process of a scenario that sets none of
/// these, so that scenario's manifests name no plugin at all. Otherwise the container also names each plugin it relies
/// on as required, so it is refused on a node where one is missing.
fn runtime_annotations(container: &str, limits: &ResolvedLimits) -> BTreeMap<String, String> {
    let mut annotations: BTreeMap<String, String> = cgroup_files(limits)
        .into_iter()
        .map(|(file, value)| (format!("{file}.memory-qos.nri.io/{container}"), value))
        .collect();
    if !limits.rest.ulimits.is_empty() {
        let entries = limits
            .rest
            .ulimits
            .iter()
            .fold(String::default(), |mut entries, (name, ulimit)| {
                let (soft, hard) = ulimit.soft_hard();
                let kind = name.to_ascii_uppercase();
                let _write_result = writeln!(entries, "- type: RLIMIT_{kind}\n  soft: {soft}\n  hard: {hard}");
                entries
            });
        let _previous = annotations.insert(format!("{ULIMITS_ANNOTATION}.{container}"), entries);
    }
    let required = required_plugins(limits);
    if !required.is_empty() {
        let names = required
            .iter()
            .map(|plugin| format!("\"{}\"", plugin.name))
            .collect::<Vec<_>>()
            .join(",");
        let _previous = annotations.insert(
            format!("{REQUIRED_PLUGINS_ANNOTATION}.{container}"),
            format!("[{names}]"),
        );
    }
    annotations
}

/// The cgroup v2 files memory-qos writes for a container, with their values, in [`MEMORY_QOS_FILES`] order.
fn cgroup_files(limits: &ResolvedLimits) -> Vec<(&'static str, String)> {
    let rest = &limits.rest;
    let cpu_max = match limits.cpu_cap {
        CpuCap::Quota { quota, period } => Some(format!("{quota} {period}")),
        CpuCap::Cpus(_) => None,
    };
    // Docker's swap limit counts memory and swap together, where the cgroup file counts the swap alone.
    let swap = rest
        .memswap_limit
        .map(|swap| swap.as_u64().saturating_sub(limits.memory.as_u64()).to_string());
    let shares = limits
        .explicit_shares
        .then(|| cpu_weight(limits.cpu_shares).to_string());
    let values = [
        cpu_max,
        shares,
        rest.cpuset.clone(),
        rest.blkio_weight.map(|weight| format!("default {weight}")),
        limits.memory_low.map(|low| low.as_u64().to_string()),
        swap,
        rest.pids.map(|pids| pids.to_string()),
    ];
    MEMORY_QOS_FILES
        .into_iter()
        .zip(values)
        .filter_map(|(file, value)| value.map(|value| (file, value)))
        .collect()
}

/// The plugins a container's limits rely on, in the order a required-plugins annotation lists them.
fn required_plugins(limits: &ResolvedLimits) -> Vec<&'static RuntimePlugin> {
    let mut required = Vec::new();
    if !cgroup_files(limits).is_empty() {
        required.push(&MEMORY_QOS);
    }
    if !limits.rest.ulimits.is_empty() {
        required.push(&ULIMIT_ADJUSTER);
    }
    required
}

/// Whether the scenario's limits need a runtime plugin, which is when [`CONTAINERD_TEMPLATE_FILE`] is written.
pub(crate) fn needs_containerd_template(cluster: &ClusterInput) -> bool {
    !cluster_plugins(cluster).is_empty()
}

/// Every plugin some process of the scenario relies on, each once, in name order.
///
/// An agent's limits apply to its runner processes, so an agent with no runner -- whose modules a browser runs --
/// contributes nothing; the scenario is rejected for setting limits on one.
fn cluster_plugins(cluster: &ClusterInput) -> Vec<&'static RuntimePlugin> {
    let agents = cluster
        .agents
        .iter()
        .filter(|agent| agent.runner.is_some())
        .map(|agent| &agent.limits);
    let services = &cluster.services;
    let mut needed: Vec<&RuntimePlugin> = [&services.collector.limits, &services.hub.limits]
        .into_iter()
        .chain(agents)
        .flat_map(|limits| required_plugins(&limits.resolve()))
        .collect();
    needed.sort_unstable_by_key(|plugin| plugin.name);
    needed.dedup_by_key(|plugin| plugin.name);
    needed
}

/// Render the README step that readies every machine for the plugins, or nothing when the scenario needs none.
pub(crate) fn node_preparation_note(cluster: &ClusterInput) -> String {
    let plugins = cluster_plugins(cluster);
    if plugins.is_empty() {
        return String::default();
    }
    let names = plugins
        .iter()
        .map(|plugin| format!("`{}`", plugin.name))
        .collect::<Vec<_>>()
        .join(" and ");
    format!(
        concat!(
            "### Prepare The Nodes\n\n",
            "Some of this scenario's limits have no field in the Pod API, so `k3s.yaml` runs these container\n",
            "runtime plugins on every node to write them into each container's cgroup:\n",
            "{names}.\n\n",
            "Each limited pod names the plugins it needs, and containerd refuses it on a node where one of them\n",
            "did not adjust it -- once NRI's validator is on, which k3s leaves off. Before applying `k3s.yaml`,\n",
            "copy the template below from this directory to every machine of the cluster and restart k3s there\n",
            "(`k3s-agent` on a machine that joined as an agent):\n\n",
            "```bash\n",
            "sudo install -D -m 0644 {file} {path}\n",
            "sudo systemctl restart k3s\n",
            "```\n\n"
        ),
        file = CONTAINERD_TEMPLATE_FILE,
        names = names,
        path = CONTAINERD_TEMPLATE_PATH,
    )
}

/// The documents that run each plugin any process of the scenario relies on, or none when no process relies on one.
fn runtime_plugin_documents(plugins: &[&RuntimePlugin]) -> Result<Vec<String>, CliError> {
    let mut docs = Vec::new();
    if !plugins.is_empty() {
        docs.push(document(&namespace_object(PLUGIN_NAMESPACE))?);
    }
    for plugin in plugins {
        if let Some((config, _)) = plugin.config {
            docs.push(document(&memory_qos_config(config))?);
        }
        docs.push(document(&plugin_daemon_set(plugin))?);
    }
    Ok(docs)
}

/// memory-qos's configuration, as the `ConfigMap` called `name`: the cgroup files it may write, and nothing else.
fn memory_qos_config(name: &str) -> ConfigMap {
    let files = MEMORY_QOS_FILES.iter().fold(String::default(), |mut files, file| {
        let _write_result = writeln!(files, "- {file}");
        files
    });
    ConfigMap {
        data: Some(BTreeMap::from([(
            "config.yaml".to_string(),
            format!("unifiedannotations:\n{files}"),
        )])),
        metadata: plain_meta(PLUGIN_NAMESPACE, name),
        ..ConfigMap::default()
    }
}

/// The plugin's container: its own image, command and arguments, with the NRI socket and any configuration mounted.
///
/// It runs as root because the socket is root's alone, and that is the only privilege it keeps: every capability is
/// dropped and the root filesystem is read-only, so the socket is the one thing it can reach that an ordinary container
/// cannot.
fn plugin_container(plugin: &RuntimePlugin) -> (Container, Vec<Volume>) {
    let owned = |items: &[&str]| items.iter().map(|item| (*item).to_string()).collect::<Vec<_>>();
    let (socket_mount, socket_volume) = host_socket("nri-socket", NRI_SOCKET);
    let mut mounts = vec![socket_mount];
    let mut volumes = vec![socket_volume];
    if let Some((config, path)) = plugin.config {
        mounts.push(volume_mount(config, path));
        volumes.push(Volume {
            config_map: Some(ConfigMapVolumeSource {
                name: config.to_string(),
                ..ConfigMapVolumeSource::default()
            }),
            name: config.to_string(),
            ..Volume::default()
        });
    }
    let container = Container {
        args: Some(owned(plugin.args)),
        command: (!plugin.command.is_empty()).then(|| owned(plugin.command)),
        image: Some(plugin.image.to_string()),
        image_pull_policy: Some(IMAGE_PULL_POLICY.to_string()),
        name: plugin.workload.to_string(),
        resources: Some(container_resources(&ResolvedLimits::default())),
        security_context: Some(SecurityContext {
            run_as_non_root: Some(false),
            run_as_user: Some(0),
            ..hardened_container_context()
        }),
        volume_mounts: Some(mounts),
        ..Container::default()
    };
    (container, volumes)
}

/// One plugin, on every node, connected to the node's NRI socket.
///
/// Node-critical priority keeps the scheduler from evicting it under load, which would leave every limited container on
/// the node refused until it returned.
fn plugin_daemon_set(plugin: &RuntimePlugin) -> DaemonSet {
    let (container, volumes) = plugin_container(plugin);
    DaemonSet {
        metadata: meta(PLUGIN_NAMESPACE, plugin.workload),
        spec: Some(DaemonSetSpec {
            selector: LabelSelector {
                match_labels: Some(labels(plugin.workload)),
                ..LabelSelector::default()
            },
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(labels(plugin.workload)),
                    ..ObjectMeta::default()
                }),
                spec: Some(PodSpec {
                    containers: vec![container],
                    priority_class_name: Some("system-node-critical".to_string()),
                    security_context: Some(PodSecurityContext {
                        seccomp_profile: Some(SeccompProfile {
                            type_: "RuntimeDefault".to_string(),
                            ..SeccompProfile::default()
                        }),
                        ..PodSecurityContext::default()
                    }),
                    volumes: Some(volumes),
                    ..PodSpec::default()
                }),
            },
            ..DaemonSetSpec::default()
        }),
        ..DaemonSet::default()
    }
}

/// Mount a socket of the node's at the same path in the container.
fn host_socket(name: &str, path: &str) -> (VolumeMount, Volume) {
    let volume = Volume {
        host_path: Some(HostPathVolumeSource {
            path: path.to_string(),
            type_: Some("Socket".to_string()),
        }),
        name: name.to_string(),
        ..Volume::default()
    };
    (volume_mount(name, path), volume)
}

/// Size the container's `/dev/shm` as the scenario asked, with a memory-backed volume of that size over it.
///
/// Kubernetes has no shared-memory setting of its own: the runtime's default is a fixed 64MiB, and a memory-medium
/// `emptyDir` mounted over the path is how a pod gets any other size.
fn mount_shm(container: &mut Container, volumes: &mut Vec<Volume>, limits: &Limits) {
    let Some(size) = limits.shm_size else {
        return;
    };
    volumes.push(Volume {
        empty_dir: Some(EmptyDirVolumeSource {
            medium: Some("Memory".to_string()),
            size_limit: Some(Quantity(k8s_bytes(size))),
        }),
        name: SHM_VOLUME.to_string(),
        ..Volume::default()
    });
    container
        .volume_mounts
        .get_or_insert_with(Vec::new)
        .push(volume_mount(SHM_VOLUME, "/dev/shm"));
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
/// `path` is a container mount point in the manifest this generator emits, not a path anything in this process opens,
/// so a caller passing `/tmp` is naming the containerised program's own temp directory rather than creating a world-
/// writable file on the host. `DeepSource RS-S1003` reads the literal as the latter, which is why every call site
/// passing `/tmp` carries a `skipcq` for that one rule.
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

/// Wrap a container in the single-replica Deployment every component here uses, pinned to the machine at `site`.
///
/// A single-host scenario pins nothing and leaves placement to the scheduler, which on one node has no choice to make.
/// `limits` are applied here rather than by each caller, so no component can be built without them.
fn deployment(
    namespace: &str,
    name: &str,
    container: Container,
    volumes: Vec<Volume>,
    site: &Site,
    limits: &Limits,
) -> Deployment {
    Deployment {
        metadata: meta(namespace, name),
        spec: Some(DeploymentSpec {
            replicas: Some(1),
            selector: LabelSelector {
                match_labels: Some(labels(name)),
                ..LabelSelector::default()
            },
            template: pod_template(name, container, volumes, site, limits),
            ..DeploymentSpec::default()
        }),
        ..Deployment::default()
    }
}

/// The pod every workload here runs: one container under `limits`, pinned to the machine at `site`.
fn pod_template(
    name: &str,
    mut container: Container,
    mut volumes: Vec<Volume>,
    site: &Site,
    limits: &Limits,
) -> PodTemplateSpec {
    let resolved = limits.resolve();
    container.resources = Some(container_resources(&resolved));
    mount_shm(&mut container, &mut volumes, limits);
    let annotations = runtime_annotations(&container.name, &resolved);
    let node_selector: BTreeMap<String, String> = site
        .machine_names()
        .into_iter()
        .map(|machine| (MACHINE_LABEL.to_string(), machine))
        .collect();
    PodTemplateSpec {
        metadata: Some(ObjectMeta {
            annotations: (!annotations.is_empty()).then_some(annotations),
            labels: Some(labels(name)),
            ..ObjectMeta::default()
        }),
        spec: Some(PodSpec {
            containers: vec![container],
            node_selector: (!node_selector.is_empty()).then_some(node_selector),
            security_context: Some(hardened_pod_context()),
            volumes: (!volumes.is_empty()).then_some(volumes),
            ..PodSpec::default()
        }),
    }
}

/// The `CronJob` a scheduled runner runs as: a pod per tick, each running its module once.
///
/// Each pod runs under the same limits and placement a continuous runner's `Deployment` gives it. `Forbid` skips a tick
/// that finds the previous run still going, the one concurrency policy every format that schedules a runner can honour.
/// The pod is never restarted in place: a failed run is retried as a new pod, up to the Job's default backoff limit.
fn cron_job(namespace: &str, name: &str, template: PodTemplateSpec, schedule: &Schedule) -> CronJob {
    let mut template = template;
    if let Some(spec) = template.spec.as_mut() {
        spec.restart_policy = Some("Never".to_string());
    }
    CronJob {
        metadata: meta(namespace, name),
        spec: Some(CronJobSpec {
            concurrency_policy: Some("Forbid".to_string()),
            job_template: JobTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(labels(name)),
                    ..ObjectMeta::default()
                }),
                spec: Some(JobSpec {
                    template,
                    ..JobSpec::default()
                }),
            },
            schedule: schedule.cron.trim().to_string(),
            time_zone: (!schedule.time_zone.is_empty()).then(|| schedule.time_zone.clone()),
            ..CronJobSpec::default()
        }),
        ..CronJob::default()
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

/// A port the container listens on, the half of a [`port`] pair the pod spec declares.
fn container_port(number: u16) -> ContainerPort {
    ContainerPort {
        container_port: i32::from(number),
        ..ContainerPort::default()
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
fn collector_deployment(namespace: &str, site: &Site, limits: &Limits) -> Deployment {
    let (data_mount, data_volume) = mount("openobserve-data", "/data");
    // The collector writes transient state outside its data directory, which a read-only root would refuse.
    // skipcq: RS-S1003
    let (tmp_mount, tmp_volume) = scratch("openobserve-tmp", "/tmp");
    let container = Container {
        security_context: Some(hardened_container_context()),
        env_from: Some(vec![from_config_map("openobserve-config"), from_secret(namespace)]),
        image: Some(OPENOBSERVE_IMAGE.to_string()),
        image_pull_policy: Some(IMAGE_PULL_POLICY.to_string()),
        name: COLLECTOR_SERVICE.to_string(),
        ports: Some(vec![container_port(Services::OtlpCollector.port())]),
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
    deployment(
        namespace,
        COLLECTOR_SERVICE,
        container,
        vec![data_volume, tmp_volume],
        site,
        limits,
    )
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
/// The paths under the runtime directory are the writable ones: a read-only root filesystem cannot take the self-signed
/// certificate the hub generates on first start, so both halves are redirected to the scratch mount.
fn hub_env(module_paths: &[String], root_module: &str) -> Vec<EnvVar> {
    // `MODULES_ROOT` is stated even when it is empty, which is what a headless cluster needs rather than the entry
    // being left out. The hub image names the page it bundles, and a container inherits an image's environment for
    // every variable the manifest does not set -- so omitting it would serve a page this cluster was not given the
    // modules for, which the hub rejects at startup. Empty is read as unset.
    let mut vars = vec![
        env("MODULES_PATHS", module_paths.join(",")),
        env("MODULES_ROOT", root_module.to_string()),
    ];
    vars.extend([
        // The account, stated here; only the password it presents comes from the `Secret`.
        env("OTLP_AUTH_USERNAME", COLLECTOR_USERNAME.to_string()),
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
    ]);
    vars
}

/// The hub, running this scenario's image because that is what carries its module set.
fn hub_deployment(
    namespace: &str,
    cluster_name: &str,
    module_paths: &[String],
    root_module: &str,
    site: &Site,
    limits: &Limits,
) -> Deployment {
    let insecure = Services::InsecureWebSocketServer.port();
    let (storage_mount, storage_volume) = mount("ws-server-storage", "/app/storage");
    let (runtime_mount, runtime_volume) = scratch("ws-server-runtime", HUB_RUNTIME_DIR);
    let container = Container {
        // `--agent-registry` is a flag rather than an environment variable, so the registry path is the one writable
        // location that has to be redirected through `args` instead of `env`.
        //
        // `command` has to be restated with it. The image declares its binary as `CMD` with no `ENTRYPOINT`, and
        // Kubernetes `args` overrides `CMD` outright rather than appending to it -- so args alone left the kubelet
        // trying to exec the flag as the program: exec: "--agent-registry": executable file not found in $PATH
        args: Some(vec![
            "--agent-registry".to_string(),
            format!("{HUB_RUNTIME_DIR}/registry.yaml"),
        ]),
        command: Some(vec!["et-ws-server".to_string()]),
        env: Some(hub_env(module_paths, root_module)),
        env_from: Some(vec![from_secret(namespace)]),
        image: Some(format!("et-ws-server-{cluster_name}:{IMAGE_TAG}")),
        image_pull_policy: Some(IMAGE_PULL_POLICY.to_string()),
        name: HUB_SERVICE.to_string(),
        ports: Some(vec![
            container_port(insecure),
            container_port(Services::SecureWebSocketServer.port()),
        ]),
        readiness_probe: Some(probe(
            None,
            Some(HTTPGetAction {
                path: Some("/health".to_string()),
                port: IntOrString::Int(i32::from(insecure)),
                ..HTTPGetAction::default()
            }),
        )),
        security_context: Some(hardened_container_context()),
        volume_mounts: Some(vec![storage_mount, runtime_mount]),
        ..Container::default()
    };
    deployment(
        namespace,
        HUB_SERVICE,
        container,
        vec![storage_volume, runtime_volume],
        site,
        limits,
    )
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

/// What wires a runner to its module and its hub, plus whatever the scenario declared for that agent.
///
/// The scenario's entries cannot collide with the derived two: the resolver rejects an `env:` naming either.
fn runner_env(runner: &RunnerInstance) -> Vec<EnvVar> {
    let mut vars = vec![
        env("RUNNER_MODULE", runner.module.clone()),
        env("WS_SERVER_URL", hub_service_ws_url()),
    ];
    vars.extend(runner.env.iter().map(|(name, value)| env(name, value.clone())));
    vars
}

/// One workload per runner, each hosting a single module exactly as the other formats arrange it.
///
/// A `Deployment` for a runner that runs continuously, a `CronJob` for a scheduled one.
fn runner_document(namespace: &str, runner: &RunnerInstance, images: ArtifactSource) -> Result<String, CliError> {
    // A runner fetches its module to a scratch directory and, for the web runner, lets Deno cache there, so a read-only
    // root needs somewhere writable even though nothing is meant to persist. skipcq: RS-S1003
    let (runtime_mount, runtime_volume) = scratch("runner-tmp", "/tmp");
    let container = Container {
        env: Some(runner_env(runner)),
        image: Some(format!(
            "{}et-ws-{}-runner:{IMAGE_TAG}",
            image_prefix(images),
            runner.runner
        )),
        image_pull_policy: Some(IMAGE_PULL_POLICY.to_string()),
        name: runner.name.clone(),
        security_context: Some(hardened_container_context()),
        volume_mounts: Some(vec![runtime_mount]),
        ..Container::default()
    };
    let volumes = vec![runtime_volume];
    if runner.schedule.is_unset() {
        return document(&deployment(
            namespace,
            &runner.name,
            container,
            volumes,
            &runner.site,
            &runner.limits,
        ));
    }
    let template = pod_template(&runner.name, container, volumes, &runner.site, &runner.limits);
    document(&cron_job(namespace, &runner.name, template, &runner.schedule))
}
