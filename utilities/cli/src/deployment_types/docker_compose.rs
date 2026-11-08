use std::collections::BTreeMap;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};

use edge_toolkit::ports::Services;
use et_path::{absolute_from, relative_path_from};
use fs_err as fs;
use serde::ser::SerializeMap as _;
use serde::{Serialize, Serializer};

use super::hub_sharing::{
    BACKPLANE_HEALTH_PATH, BACKPLANE_IMAGE, BACKPLANE_TASK, DATA_DIR, SHARED_STORAGE_HEALTH_PATH, SHARED_STORAGE_IMAGE,
    SHARED_STORAGE_TASK, backplane_args, shared_hub_env, shared_storage_env, shared_storage_start,
};
use super::{COLLECTOR_TASK, REGISTRY_TOKEN_ENV, REGISTRY_TOKEN_SECRET, scenario_image_needs};
use crate::error::CliError;
use crate::input::{ArtifactSource, ClusterInput};
use crate::limits::{CpuCap, ResolvedLimits, compose_bytes};
use crate::placement::{Hub, hub_site, hubs, shares_hubs};
use crate::{
    COLLECTOR_SETTINGS, COLLECTOR_USERNAME, IMAGE_REGISTRY, OutputType, RunnerInstance, SECRETS_ENV_FILE,
    cluster_module_names, hub_http_base, module_path_contexts, module_registry, resolve_cluster_runners,
    resolve_module_paths,
};

/// The compose service that only builds the hub image a local scenario's `FROM hub` layers onto.
///
/// Compose orders one build after another only through `additional_contexts: <name>: service:<service>`, so the hub
/// build has to be a service to be referenced at all. `scale: 0` keeps compose from also running it, and `profiles:`
/// cannot do that instead, since a service hidden by a profile is unknown to the build that references it.
pub(crate) const HUB_IMAGE_BUILD: &str = "hub-image-build";

/// The compose service that starts scheduled runners on their schedules, on a single-host cluster.
pub(crate) const SCHEDULER_SERVICE: &str = "scheduler";

/// The Ofelia image the scheduler runs: netresearch's maintained fork, whose tags carry no `v`.
const OFELIA_IMAGE: &str = "ghcr.io/netresearch/ofelia:1.0.1";

/// The label naming the scenario a container belongs to, which the scheduler reads job definitions through.
///
/// Ofelia reads labels off every container on the daemon unless filtered, so without it two scenarios sharing a Docker
/// host would each schedule the other's jobs.
const SCENARIO_LABEL: &str = concat!(et_org::org!(), "/scenario");

/// The scheduler service for the machine `machines` names, or for the single host when it names none.
///
/// One per machine with a scheduled runner, since Ofelia can only start a container on the Docker daemon it runs on.
pub(crate) fn scheduler_service(machines: &[String]) -> String {
    match machines {
        [machine] => format!("{SCHEDULER_SERVICE}-{machine}"),
        _ => SCHEDULER_SERVICE.to_string(),
    }
}

/// The container a scheduled runner's service runs as, named so the scheduler can start it again on each tick.
///
/// Scoped to the scenario, because a container name is unique across the whole Docker daemon.
fn scheduled_container(cluster_name: &str, runner: &str) -> String {
    format!("et-{cluster_name}-{runner}")
}

/// The [`HUB_IMAGE_BUILD`] service, building the hub from `context`.
#[expect(
    clippy::single_call_fn,
    reason = "the hub-build step of generate_docker_compose_deployment"
)]
fn hub_build_service(context: String) -> ComposeService {
    ComposeService {
        build: Some(ComposeBuild {
            source: BuildSource::Within {
                context,
                dockerfile: "services/ws-server/Dockerfile".to_string(),
            },
            additional_contexts: Vec::new(),
            secrets: Vec::new(),
        }),
        scale: Some(0),
        ..ComposeService::default()
    }
}

/// A `depends_on` entry that waits for `service` to report healthy, not merely to have started.
fn healthy(service: &str) -> (String, ComposeDependsOnCondition) {
    let condition = ComposeDependsOnCondition {
        condition: "service_healthy".to_string(),
        required: true,
    };
    (service.to_string(), condition)
}

/// The hub's own readiness probe, which the runners gate on.
///
/// A runner resolves its module by fetching `/modules/<name>/package.json`, which fails outright instead of retrying,
/// so "container started" is not a strong enough edge -- `service_started` would let a runner ask before the listener
/// exists.
#[expect(
    clippy::single_call_fn,
    reason = "the healthcheck step of generate_docker_compose_deployment"
)]
fn hub_healthcheck() -> ComposeHealthcheck {
    curl_healthcheck(&format!("{}/health", hub_http_base()))
}

/// A healthcheck that passes once `url` answers with a success status, asked with the image's own `curl`.
fn curl_healthcheck(url: &str) -> ComposeHealthcheck {
    healthcheck(&["CMD", "curl", "-fsS", url])
}

/// A healthcheck running `test`, on the cadence every service here shares.
fn healthcheck(test: &[&str]) -> ComposeHealthcheck {
    ComposeHealthcheck {
        test: test.iter().map(|word| (*word).to_string()).collect(),
        interval: "5s".to_string(),
        timeout: "3s".to_string(),
        retries: 20,
        start_period: "10s".to_string(),
    }
}

/// The hub service running this scenario's own image, and the top-level secrets its build reads.
///
/// The image builds from this directory unless it copies something out of the repository, which a published scenario's
/// modules never are: they come from their releases, read with the token the secret carries.
#[expect(
    clippy::single_call_fn,
    reason = "the scenario-image step of generate_docker_compose_deployment"
)]
fn scenario_hub_service(
    cluster: &ClusterInput,
    output_abs: &Path,
    hub_contexts: Vec<(String, String)>,
    hub: &Hub,
) -> Result<(ComposeService, Vec<String>), CliError> {
    let workspace_root = edge_toolkit::config::get_project_root();
    let serves_a_page = super::serves_a_page(cluster);
    let module_paths = docker_image_module_paths(&cluster_module_names(cluster), serves_a_page, &cluster.module_paths)?;
    let needs = scenario_image_needs(cluster)?;
    let source = if needs.repository {
        // `dockerfile` is resolved against the build context, not against this compose file, so the scenario image is
        // named by its path down from the repository root rather than as a sibling of the compose file.
        BuildSource::Within {
            context: relative_path_from(output_abs, &workspace_root),
            dockerfile: format!("{}/Dockerfile", relative_path_from(&workspace_root, output_abs)),
        }
    } else {
        BuildSource::Here
    };
    let secrets = if needs.registry_token {
        vec![REGISTRY_TOKEN_SECRET.to_string()]
    } else {
        Vec::new()
    };
    let shared = shares_hubs(cluster);
    let mut environment = hub_environment(&module_paths, &workspace_root, serves_a_page, shared);
    let mut depends_on = vec![healthy(COLLECTOR_TASK)];
    if shared {
        environment.extend(
            shared_hub_env(&hub.server_host, &hub.server_host)
                .into_iter()
                .map(|(name, value)| (name.to_string(), value)),
        );
        environment.sort_by(|(left, _), (right, _)| left.cmp(right));
        depends_on.extend([healthy(BACKPLANE_TASK), healthy(SHARED_STORAGE_TASK)]);
    }
    // A hub off the server cannot be gated on services its own compose project does not start, so the edges are kept
    // but waived, and the hub is restarted until they answer instead, as a runner off the server is.
    if !hub.on_server() {
        for (_, condition) in &mut depends_on {
            condition.required = false;
        }
    }
    let service = ComposeService {
        profiles: hub.site.machine_names(),
        build: Some(ComposeBuild {
            source,
            // Relative to this file, which is what compose resolves a context path against.
            additional_contexts: hub_contexts
                .into_iter()
                .chain(module_path_contexts(&cluster.module_paths, output_abs))
                .collect(),
            secrets: secrets.clone(),
        }),
        restart: (!hub.on_server()).then(|| "on-failure".to_string()),
        network_mode: Some("host".to_string()),
        // Carries `OTLP_AUTH_PASSWORD`: the server authenticates its OTLP exports against the same root credential the
        // collector was started with. The account it authenticates as is in the environment.
        env_file: vec![SECRETS_ENV_FILE.to_string()],
        environment,
        volumes: if shared {
            Vec::new()
        } else {
            vec!["ws-server-storage:/app/storage".to_string()]
        },
        depends_on,
        healthcheck: Some(hub_healthcheck()),
        limits: cluster.services.hub.limits.resolve(),
        ..ComposeService::default()
    };
    Ok((service, secrets))
}

/// The NATS and rustfs services the hubs share, reachable at `localhost` on the server and at its address elsewhere.
///
/// rustfs takes host networking, as the hub does. NATS publishes its ports instead, as the mise task does: its client
/// port to every interface for the other machines' hubs, and its monitoring port to loopback alone, since only the
/// healthcheck reads it and NATS has no setting that binds that listener apart from the client one.
#[expect(
    clippy::single_call_fn,
    reason = "the shared-services step of generate_docker_compose_deployment"
)]
fn shared_services(cluster: &ClusterInput, profiles: &[String]) -> [(String, ComposeService); 2] {
    let backplane = ComposeService {
        profiles: profiles.to_vec(),
        image: Some(BACKPLANE_IMAGE.to_string()),
        command: backplane_args(),
        healthcheck: Some(backplane_healthcheck()),
        ports: vec![
            format!("{client}:{client}", client = Services::Backplane.port()),
            format!(
                "127.0.0.1:{monitor}:{monitor}",
                monitor = Services::BackplaneMonitor.port()
            ),
        ],
        volumes: vec![format!("{BACKPLANE_TASK}-data:{DATA_DIR}")],
        limits: cluster.services.backplane.limits.resolve(),
        ..ComposeService::default()
    };
    let storage = ComposeService {
        profiles: profiles.to_vec(),
        image: Some(SHARED_STORAGE_IMAGE.to_string()),
        entrypoint: vec!["sh".to_string(), "-c".to_string()],
        command: vec![shared_storage_start()],
        healthcheck: Some(curl_healthcheck(&format!(
            "http://localhost:{}{SHARED_STORAGE_HEALTH_PATH}",
            Services::SharedStorage.port()
        ))),
        network_mode: Some("host".to_string()),
        // Carries `RUSTFS_SECRET_KEY`, the secret the hubs present as `AWS_SECRET_ACCESS_KEY`.
        env_file: vec![SECRETS_ENV_FILE.to_string()],
        environment: shared_storage_env()
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect(),
        volumes: vec![format!("{SHARED_STORAGE_TASK}-data:{DATA_DIR}")],
        limits: cluster.services.storage.limits.resolve(),
        ..ComposeService::default()
    };
    [
        (BACKPLANE_TASK.to_string(), backplane),
        (SHARED_STORAGE_TASK.to_string(), storage),
    ]
}

/// The NATS server's healthcheck, which passes once its health endpoint answers `200` with `JetStream` up.
///
/// The image ships no `curl`, so the request is written by hand and sent with its `nc`, and the status line read back.
#[expect(
    clippy::single_call_fn,
    reason = "the NATS step of shared_services, separate to keep the request out of the literal"
)]
fn backplane_healthcheck() -> ComposeHealthcheck {
    let probe = format!(
        "printf 'GET {BACKPLANE_HEALTH_PATH} HTTP/1.0\\r\\n\\r\\n' | nc localhost {} | head -1 | grep -q ' 200 '",
        Services::BackplaneMonitor.port()
    );
    healthcheck(&["CMD-SHELL", &probe])
}

/// One service per hub, and the top-level secrets their builds read.
///
/// A local scenario also gets the service building the hub image it layers onto, on every machine with a hub; a
/// published scenario's Dockerfile names the released image itself.
#[expect(
    clippy::single_call_fn,
    reason = "the hub step of generate_docker_compose_deployment"
)]
fn hub_services(
    cluster: &ClusterInput,
    output_abs: &Path,
    workspace_rel: String,
    published: bool,
) -> Result<(Vec<NamedService>, Vec<String>), CliError> {
    let hubs = hubs(cluster);
    let mut services = Vec::new();
    let hub_contexts = if published {
        Vec::new()
    } else {
        services.push((
            HUB_IMAGE_BUILD.to_string(),
            ComposeService {
                limits: cluster.services.hub.limits.resolve(),
                profiles: hubs.iter().flat_map(|hub| hub.site.machine_names()).collect(),
                ..hub_build_service(workspace_rel)
            },
        ));
        vec![("hub".to_string(), format!("service:{HUB_IMAGE_BUILD}"))]
    };
    let mut secrets = Vec::new();
    for hub in &hubs {
        let (service, hub_secrets) = scenario_hub_service(cluster, output_abs, hub_contexts.clone(), hub)?;
        services.push((hub.name.clone(), service));
        secrets = hub_secrets;
    }
    Ok((services, secrets))
}

pub fn generate_docker_compose_deployment(cluster: &ClusterInput, output_dir: &Path) -> Result<(), CliError> {
    let output_path = output_dir.join(OutputType::DockerCompose.output_file_name());
    let workspace_root = edge_toolkit::config::get_project_root();
    let output_abs = absolute_from(&workspace_root, output_dir);
    let workspace_rel = relative_path_from(&output_abs, &workspace_root);
    let artifacts = cluster.artifact_source;
    let published = matches!(artifacts, ArtifactSource::Published);
    // Everything but the runners and the other machines' hubs runs on the server machine.
    let server_profiles = hub_site(cluster).machine_names();
    let shared = shares_hubs(cluster);
    let mut services = vec![(
        COLLECTOR_TASK.to_string(),
        ComposeService {
            profiles: server_profiles.clone(),
            limits: cluster.services.collector.limits.resolve(),
            ..openobserve_service()
        },
    )];
    if shared {
        services.extend(shared_services(cluster, &server_profiles));
    }
    let (hub_services, secrets) = hub_services(cluster, &output_abs, workspace_rel, published)?;
    services.extend(hub_services);
    let runners = resolve_cluster_runners(
        &module_registry(
            &workspace_root,
            &workspace_root.join("services/ws-server"),
            &cluster.module_paths,
        ),
        cluster,
    )?;
    services.extend(runner_services(
        cluster,
        &runners,
        &relative_path_from(&output_abs, &workspace_root),
        artifacts,
    ));
    services.extend(scheduler_services(&runners, &cluster.cluster_name));
    let volumes = if shared {
        vec![
            format!("{BACKPLANE_TASK}-data"),
            "openobserve-data".to_string(),
            format!("{SHARED_STORAGE_TASK}-data"),
        ]
    } else {
        vec!["openobserve-data".to_string(), "ws-server-storage".to_string()]
    };
    let compose = ComposeFile {
        services,
        volumes: volumes.into_iter().map(|name| (name, ComposeVolume)).collect(),
        secrets,
    };
    fs::write(&output_path, render_compose_yaml(&compose)?)?;

    Ok(())
}

/// Build one service per runner the cluster declares, each hosting a single module.
///
/// `network_mode: host` matches the hub's, and that pairing is what makes the URLs below resolve: the hub puts its
/// listener on the host rather than on a compose network, so a runner on the default bridge would have no route to
/// it that is portable across platforms. Sharing the host namespace instead means `localhost` means the same thing in
/// both containers.
///
/// The dependency is on the hub being HEALTHY, not merely started -- a runner resolves its module over HTTP and fails
/// outright rather than retrying, so an early start is a lost run rather than a slow one.
///
/// Nothing about a runner image varies by scenario, so a published one is named rather than built. That is the same
/// image the Kubernetes manifests name, and the same binary the `mise` deployment installs from crates.io.
///
/// A scheduled runner's service is started once by `up` like any other, and then again by the scheduler on each tick;
/// its container is named, labelled with the scenario, and never restarted by compose, since its next tick is its
/// retry.
fn runner_services(
    cluster: &ClusterInput,
    runners: &[RunnerInstance],
    context: &str,
    artifacts: ArtifactSource,
) -> Vec<(String, ComposeService)> {
    let published = matches!(artifacts, ArtifactSource::Published);
    let cluster_name = cluster.cluster_name.as_str();
    runners
        .iter()
        .map(|runner| {
            let kind = &runner.runner;
            // A runner on a machine without a hub cannot be gated on a hub its own compose project does not start, so
            // the edge is kept but waived, and the runner is restarted until the hub answers instead.
            let remote = runner.site.is_remote();
            let scheduled = !runner.schedule.is_unset();
            let (name, mut hub) = healthy(&runner.site.hub_name(cluster));
            hub.required = !remote;
            let service = ComposeService {
                container_name: scheduled.then(|| scheduled_container(cluster_name, &runner.name)),
                labels: if scheduled {
                    vec![(SCENARIO_LABEL.to_string(), cluster_name.to_string())]
                } else {
                    Vec::new()
                },
                profiles: runner.site.machine_names(),
                build: (!published).then(|| ComposeBuild {
                    source: BuildSource::Within {
                        context: context.to_string(),
                        dockerfile: format!("services/ws-{kind}-runner/Dockerfile"),
                    },
                    additional_contexts: Vec::new(),
                    secrets: Vec::new(),
                }),
                restart: (remote && !scheduled).then(|| "on-failure".to_string()),
                image: published.then(|| format!("{IMAGE_REGISTRY}/et-ws-{kind}-runner:latest")),
                network_mode: Some("host".to_string()),
                environment: runner_environment(runner),
                depends_on: vec![(name, hub)],
                limits: runner.limits.resolve(),
                ..ComposeService::default()
            };
            (runner.name.clone(), service)
        })
        .collect()
}

/// One Ofelia service per machine with a scheduled runner, starting each runner's container on its schedule.
///
/// Every job is declared as labels on the scheduler itself, which is where Ofelia reads a `job-run` from, and the
/// scheduler reads only containers carrying this scenario's label -- its own included. A job names its runner's
/// container rather than an image, so a tick runs the very container compose created, with its environment, network,
/// limits and image intact; `docker start` is all Ofelia adds. `no-overlap` skips a tick that finds the previous run
/// still going. A time zone is given in robfig's `CRON_TZ=` prefix, the only way its parser takes one.
///
/// The Docker socket is mounted because starting a container is a Docker API call; read-only on the mount does not
/// limit what that API allows, so the scheduler is as privileged as the daemon. Only a scenario with a schedule gets
/// one. `--config=/dev/null` stands in for the config file Ofelia otherwise looks for, since every job is a label.
fn scheduler_services(runners: &[RunnerInstance], cluster_name: &str) -> Vec<(String, ComposeService)> {
    let mut sites: Vec<Vec<String>> = Vec::new();
    for runner in runners.iter().filter(|runner| !runner.schedule.is_unset()) {
        let machines = runner.site.machine_names();
        if !sites.contains(&machines) {
            sites.push(machines);
        }
    }
    sites
        .into_iter()
        .map(|machines| {
            let mut labels = vec![
                (SCENARIO_LABEL.to_string(), cluster_name.to_string()),
                ("ofelia.enabled".to_string(), "true".to_string()),
            ];
            labels.extend(
                runners
                    .iter()
                    .filter(|runner| !runner.schedule.is_unset() && runner.site.machine_names() == machines)
                    .flat_map(|runner| job_labels(runner, cluster_name)),
            );
            let service = ComposeService {
                image: Some(OFELIA_IMAGE.to_string()),
                command: vec![
                    "daemon".to_string(),
                    "--config=/dev/null".to_string(),
                    "--docker-filter".to_string(),
                    format!("label={SCENARIO_LABEL}={cluster_name}"),
                ],
                volumes: vec!["/var/run/docker.sock:/var/run/docker.sock:ro".to_string()],
                labels,
                profiles: machines.clone(),
                ..ComposeService::default()
            };
            (scheduler_service(&machines), service)
        })
        .collect()
}

/// The scheduler labels that declare one scheduled runner's job: when, which container, and that runs never overlap.
fn job_labels(runner: &RunnerInstance, cluster_name: &str) -> [(String, String); 3] {
    let job = format!("ofelia.job-run.{}", runner.name);
    let cron = runner.schedule.cron.trim();
    let schedule = if runner.schedule.time_zone.is_empty() {
        cron.to_string()
    } else {
        format!("CRON_TZ={} {cron}", runner.schedule.time_zone)
    };
    [
        (format!("{job}.schedule"), schedule),
        (
            format!("{job}.container"),
            scheduled_container(cluster_name, &runner.name),
        ),
        (format!("{job}.no-overlap"), "true".to_string()),
    ]
}

/// What wires a runner to its module and its hub, plus whatever the scenario declared for that agent.
///
/// The scenario's entries cannot collide with the derived two: the resolver rejects an `env:` naming either.
fn runner_environment(runner: &RunnerInstance) -> Vec<(String, String)> {
    let mut environment = vec![
        ("RUNNER_MODULE".to_string(), runner.module.clone()),
        ("WS_SERVER_URL".to_string(), runner.site.hub_ws_url()),
    ];
    environment.extend(runner.env.iter().map(|(name, value)| (name.clone(), value.clone())));
    environment
}

/// The collector's non-secret settings, plus the data directory the named volume below makes meaningful.
fn collector_environment() -> Vec<(String, String)> {
    let mut environment: Vec<(String, String)> = COLLECTOR_SETTINGS
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect();
    environment.push(("ZO_DATA_DIR".to_string(), "/data".to_string()));
    environment
}

/// Build the `OpenObserve` collector service the scenario's traces are exported to.
fn openobserve_service() -> ComposeService {
    ComposeService {
        image: Some("openobserve/openobserve:v1.1.2".to_string()),
        healthcheck: Some(healthcheck(&["CMD", "/openobserve", "node", "status"])),
        // Bound to loopback, not every interface. A collector published on 0.0.0.0 would hand its root login to anyone
        // who can reach the host and read the scenario's generated env file. Nothing outside the developer's machine
        // needs to talk to it: the ws-server exports to 127.0.0.1:5080 and the UI is opened locally.
        ports: vec!["127.0.0.1:5080:5080".to_string()],
        // The scenario's own file is the only one, and it carries only the credential. Per-scenario credentials mean
        // two stacks running side by side cannot authenticate against each other's collector.
        env_file: vec![SECRETS_ENV_FILE.to_string()],
        environment: collector_environment(),
        volumes: vec!["openobserve-data:/data".to_string()],
        ..ComposeService::default()
    }
}

/// The hub service's environment, in the alphabetical order the rest of the file is written in.
///
/// `MODULES_ROOT` is stated even when it is empty, which is what a headless cluster needs rather than the entry
/// being left out. The hub image names the page it bundles, and a container inherits an image's environment for every
/// variable the compose file does not set -- so omitting it would serve a page this cluster was not given the modules
/// for, which the hub rejects at startup. Empty is read as unset, and is quoted as YAML's empty string, since written
/// bare it would be a null, which compose reads as "take this from the host environment".
///
/// `MODULES_PATHS` is the comma-and-space list the server's per-segment trim expects, and what the `mise` deployment
/// writes too.
#[expect(
    clippy::single_call_fn,
    reason = "distinct step of the hub service; separate so the root's two spellings are not inlined mid-literal"
)]
fn hub_environment(
    module_paths: &[String],
    workspace_root: &Path,
    serves_a_page: bool,
    shared: bool,
) -> Vec<(String, String)> {
    let root_module = if serves_a_page {
        super::hub_root_module(&workspace_root.join("services/ws-server"))
    } else {
        String::default()
    };
    let mut environment = vec![
        ("MODULES_PATHS".to_string(), module_paths.join(", ")),
        ("MODULES_ROOT".to_string(), root_module),
        ("OTLP_AUTH_USERNAME".to_string(), COLLECTOR_USERNAME.to_string()),
        (
            "OTLP_COLLECTOR_URL".to_string(),
            "http://127.0.0.1:5080/api/default/v1".to_string(),
        ),
    ];
    // Hubs that share storage are pointed at it by the caller instead.
    if !shared {
        environment.push(("STORAGE_URL".to_string(), "file:///app/storage".to_string()));
    }
    environment
}

pub fn docker_image_module_paths(
    module_names: &[String],
    serves_a_page: bool,
    module_paths: &[PathBuf],
) -> Result<Vec<String>, CliError> {
    let project_root = edge_toolkit::config::get_project_root();
    let ws_server_dir = project_root.join("services/ws-server");
    // The page is resolved by name like any other module, so the agent and runtimes it declares come with it.
    let mut names = Vec::with_capacity(module_names.len().saturating_add(1));
    if serves_a_page {
        names.push("static".to_string());
    }
    names.extend(module_names.iter().cloned());
    let registry = module_registry(&project_root, &ws_server_dir, module_paths);
    resolve_module_paths(&registry, &names, |entry| entry.docker_path.clone())
}

/// One service of the file, under the name it is written as.
type NamedService = (String, ComposeService);

/// The whole compose file, each field a top-level key.
#[derive(Debug, Default, Serialize)]
struct ComposeFile {
    #[serde(serialize_with = "map_of")]
    services: Vec<NamedService>,
    #[serde(serialize_with = "map_of")]
    volumes: Vec<(String, ComposeVolume)>,
    /// Build secrets, by name; each is the registry token, read from the host's environment.
    #[serde(serialize_with = "registry_token_secrets", skip_serializing_if = "Vec::is_empty")]
    secrets: Vec<String>,
}

/// One service, its fields declared in the order the file writes them.
#[derive(Debug, Default, Serialize)]
struct ComposeService {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    profiles: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image: Option<String>,
    /// A fixed container name, for a container something outside compose has to find by name.
    #[serde(skip_serializing_if = "Option::is_none")]
    container_name: Option<String>,
    /// The command replacing the image's `ENTRYPOINT`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    entrypoint: Vec<String>,
    /// Arguments replacing the image's `CMD`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    command: Vec<String>,
    /// Container labels, in the order they are written.
    #[serde(serialize_with = "map_of", skip_serializing_if = "Vec::is_empty")]
    labels: Vec<(String, String)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    healthcheck: Option<ComposeHealthcheck>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    ports: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    env_file: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    build: Option<ComposeBuild>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scale: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    network_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restart: Option<String>,
    #[serde(serialize_with = "map_of", skip_serializing_if = "Vec::is_empty")]
    environment: Vec<(String, String)>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    volumes: Vec<String>,
    #[serde(serialize_with = "map_of", skip_serializing_if = "Vec::is_empty")]
    depends_on: Vec<(String, ComposeDependsOnCondition)>,
    /// The resource limits the service runs under, written as the keys compose spells them.
    #[serde(flatten, serialize_with = "compose_limits")]
    limits: ResolvedLimits,
}

#[derive(Debug)]
struct ComposeBuild {
    source: BuildSource,
    additional_contexts: Vec<(String, String)>,
    /// Top-level `secrets:` the build mounts, by name.
    secrets: Vec<String>,
}

/// Where a service's image is built from.
#[derive(Debug)]
enum BuildSource {
    /// The `Dockerfile` beside the compose file, with that directory as the context.
    ///
    /// Both are compose's defaults, so neither is written out.
    Here,
    /// A context and the Dockerfile within it, each relative to the compose file.
    Within { context: String, dockerfile: String },
}

impl Serialize for ComposeBuild {
    /// Write a service's `build:` key, as the shorthand `build: .` when nothing would sit beneath it.
    ///
    /// A bare `build:` parses as null rather than as compose's defaults of this directory and its Dockerfile.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        /// A build written out in full.
        #[derive(Serialize)]
        struct Spec<'build> {
            #[serde(skip_serializing_if = "Option::is_none")]
            context: Option<&'build str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            dockerfile: Option<&'build str>,
            #[serde(serialize_with = "map_of", skip_serializing_if = "<[_]>::is_empty")]
            additional_contexts: &'build [(String, String)],
            #[serde(skip_serializing_if = "<[_]>::is_empty")]
            secrets: &'build [String],
        }
        let (context, dockerfile) = match &self.source {
            BuildSource::Here => (None, None),
            BuildSource::Within { context, dockerfile } => (Some(context.as_str()), Some(dockerfile.as_str())),
        };
        if context.is_none() && self.additional_contexts.is_empty() && self.secrets.is_empty() {
            // Compose's own spelling of the directory holding the file, written out for it rather than resolved here.
            // nosemgrep: no-semgrep-mentions, no-ast-grep-mentions -- Required to waive the rule for this value.
            // ast-grep-ignore: no-relative-path-literal
            return serializer.serialize_str(".");
        }
        Spec {
            additional_contexts: &self.additional_contexts,
            context,
            dockerfile,
            secrets: &self.secrets,
        }
        .serialize(serializer)
    }
}

#[derive(Debug, Serialize)]
struct ComposeHealthcheck {
    test: Vec<String>,
    interval: String,
    timeout: String,
    retries: u32,
    start_period: String,
}

#[derive(Debug, Serialize)]
struct ComposeDependsOnCondition {
    condition: String,
    /// Whether compose refuses to start without the dependency; written out only when it does not.
    #[serde(skip_serializing_if = "is_true")]
    required: bool,
}

/// Whether `value` is set, which leaves a field that only ever says `false` out of the file.
#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if passes the field by reference"
)]
const fn is_true(value: &bool) -> bool {
    *value
}

/// A named volume, which needs nothing set, so it is written as an empty mapping.
#[derive(Debug, Default)]
struct ComposeVolume;

impl Serialize for ComposeVolume {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_map(Some(0))?.end()
    }
}

/// A service's resource limits as compose spells them, every default applied.
///
/// Applying the defaults is what makes the service run as the same process does under k3s. The CPU weight is
/// written as `cpu_shares` even when it comes from the reservation, because compose outside swarm ignores
/// `deploy.resources.reservations.cpus`: a container started from it keeps the default weight, where a Kubernetes
/// container requesting the same CPUs is weighted by that request. The memory reservation, by contrast, compose does
/// apply, as the cgroup's `memory.low`.
#[derive(Serialize)]
struct ComposeLimits {
    deploy: ComposeDeploy,
    #[serde(skip_serializing_if = "Option::is_none")]
    blkio_config: Option<ComposeBlkio>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cpu_period: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cpu_quota: Option<u64>,
    cpu_shares: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    cpuset: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    memswap_limit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shm_size: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    ulimits: BTreeMap<String, ComposeUlimit>,
}

#[derive(Serialize)]
struct ComposeDeploy {
    resources: ComposeResources,
}

#[derive(Serialize)]
struct ComposeResources {
    limits: ComposeResourceLimits,
    #[serde(skip_serializing_if = "Option::is_none")]
    reservations: Option<ComposeReservation>,
}

#[derive(Serialize)]
struct ComposeResourceLimits {
    #[serde(skip_serializing_if = "Option::is_none")]
    cpus: Option<String>,
    memory: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pids: Option<NonZeroU64>,
}

#[derive(Serialize)]
struct ComposeReservation {
    memory: String,
}

#[derive(Serialize)]
struct ComposeBlkio {
    weight: u16,
}

#[derive(Serialize)]
struct ComposeUlimit {
    soft: u64,
    hard: u64,
}

impl From<&ResolvedLimits> for ComposeLimits {
    fn from(limits: &ResolvedLimits) -> Self {
        let rest = &limits.rest;
        let (cpus, cpu_quota, cpu_period) = match limits.cpu_cap {
            CpuCap::Cpus(cpus) => (Some(cpus.to_string()), None, None),
            CpuCap::Quota { quota, period } => (None, Some(quota), Some(period)),
        };
        let resources = ComposeResources {
            limits: ComposeResourceLimits {
                cpus,
                memory: compose_bytes(limits.memory),
                pids: rest.pids,
            },
            reservations: limits.memory_low.map(|memory| ComposeReservation {
                memory: compose_bytes(memory),
            }),
        };
        Self {
            blkio_config: rest.blkio_weight.map(|weight| ComposeBlkio { weight }),
            cpu_period,
            cpu_quota,
            cpu_shares: limits.cpu_shares,
            cpuset: rest.cpuset.clone(),
            deploy: ComposeDeploy { resources },
            memswap_limit: rest.memswap_limit.map(compose_bytes),
            shm_size: rest.shm_size.map(compose_bytes),
            ulimits: rest
                .ulimits
                .iter()
                .map(|(name, ulimit)| {
                    let (soft, hard) = ulimit.soft_hard();
                    (name.clone(), ComposeUlimit { soft, hard })
                })
                .collect(),
        }
    }
}

/// Serialise the compose file.
///
/// With serde-saphyr rather than the `serde_yaml` the other YAML files are written with, for two things compose needs.
/// It quotes a string YAML 1.1 reads as a boolean, `yes` or `off` among them, which compose's parser still honours and
/// rejects as an environment value -- and a scenario's own `env:` can hold one. And it writes a long string as a folded
/// block scalar, broken only at spaces, so the hub's module list fits the line length and still reads back as the one
/// comma-and-space list. Sequences are indented under their key, as in every other YAML file here, and the result is
/// restyled as the others are, which settles the quotes it picks.
fn render_compose_yaml(compose: &ComposeFile) -> Result<String, CliError> {
    let mut options = serde_saphyr::SerializerOptions::default();
    options.compact_list_indent = false;
    let yaml = serde_saphyr::to_string_with_options(compose, options)?;
    Ok(super::yaml::formatted(&yaml))
}

/// Serialise `pairs` as a mapping, in the order they are listed.
fn map_of<S, V>(pairs: &[(String, V)], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    V: Serialize,
{
    serializer.collect_map(pairs.iter().map(|pair| (&pair.0, &pair.1)))
}

/// Serialise the build secrets by name, each read from the host variable the registry token is in.
fn registry_token_secrets<S>(names: &[String], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    /// A secret compose reads from the host's environment.
    #[derive(Serialize)]
    struct FromEnvironment {
        environment: &'static str,
    }
    let secret = FromEnvironment {
        environment: REGISTRY_TOKEN_ENV,
    };
    serializer.collect_map(names.iter().map(|name| (name, &secret)))
}

/// Serialise a service's resource limits as the keys compose spells them.
fn compose_limits<S>(limits: &ResolvedLimits, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    ComposeLimits::from(limits).serialize(serializer)
}
