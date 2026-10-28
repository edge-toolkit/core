use std::path::{Path, PathBuf};

use edge_toolkit::ports::Services;
use et_path::{absolute_from, relative_path_from};
use fs_err as fs;

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

/// Characters YAML reserves as indicators, which a plain scalar may not open with.
///
/// `@` and a backtick are reserved for future use and are a scanner error outright; the rest introduce anchors, tags,
/// flow collections, block scalars and the like. A value starting with any of them has to be quoted to be read back as
/// the string it is.
const YAML_INDICATORS: [char; 18] = [
    '@', '`', '-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '>', '\'', '"',
];

/// Whether YAML reads `value`, written bare, as a boolean, a null or a number rather than the string it is.
///
/// The YAML 1.1 spellings are included, since compose's parser still honours `yes`, `no`, `on` and `off`.
fn reads_as_non_string(value: &str) -> bool {
    const RESERVED: [&str; 10] = ["true", "false", "yes", "no", "on", "off", "y", "n", "null", "~"];
    RESERVED.iter().any(|reserved| value.eq_ignore_ascii_case(reserved)) || value.parse::<f64>().is_ok()
}

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
    let mut environment = hub_environment(module_paths, &workspace_root, serves_a_page, shared);
    let mut depends_on = vec![healthy(COLLECTOR_TASK)];
    if shared {
        environment.extend(
            shared_hub_env(&hub.server_host, &hub.server_host)
                .into_iter()
                .map(|(name, value)| (name.to_string(), ComposeValue::Plain(value))),
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
            .map(|(name, value)| (name.to_string(), ComposeValue::Plain(value)))
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
    let content = render_compose_yaml(&compose);
    fs::write(&output_path, content)?;

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
fn runner_environment(runner: &RunnerInstance) -> Vec<(String, ComposeValue)> {
    let mut environment = vec![
        ("RUNNER_MODULE".to_string(), ComposeValue::Plain(runner.module.clone())),
        (
            "WS_SERVER_URL".to_string(),
            ComposeValue::Plain(runner.site.hub_ws_url()),
        ),
    ];
    environment.extend(
        runner
            .env
            .iter()
            .map(|(name, value)| (name.clone(), ComposeValue::Plain(value.clone()))),
    );
    environment
}

/// The collector's non-secret settings, plus the data directory the named volume below makes meaningful.
fn collector_environment() -> Vec<(String, ComposeValue)> {
    let mut environment: Vec<(String, ComposeValue)> = COLLECTOR_SETTINGS
        .iter()
        .map(|(name, value)| ((*name).to_string(), ComposeValue::Plain((*value).to_string())))
        .collect();
    environment.push(("ZO_DATA_DIR".to_string(), ComposeValue::Plain("/data".to_string())));
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
/// for, which the hub rejects at startup. Empty is read as unset.
#[expect(
    clippy::single_call_fn,
    reason = "distinct step of the hub service; separate so the root's two spellings are not inlined mid-literal"
)]
fn hub_environment(
    module_paths: Vec<String>,
    workspace_root: &Path,
    serves_a_page: bool,
    shared: bool,
) -> Vec<(String, ComposeValue)> {
    let root_module = if serves_a_page {
        super::hub_root_module(&workspace_root.join("services/ws-server"))
    } else {
        String::default()
    };
    let mut environment = vec![
        (
            "MODULES_PATHS".to_string(),
            ComposeValue::WrappedDoubleQuoted(module_paths),
        ),
        ("MODULES_ROOT".to_string(), ComposeValue::Plain(root_module)),
    ];
    environment.extend([
        (
            "OTLP_AUTH_USERNAME".to_string(),
            ComposeValue::Plain(COLLECTOR_USERNAME.to_string()),
        ),
        (
            "OTLP_COLLECTOR_URL".to_string(),
            ComposeValue::Plain("http://127.0.0.1:5080/api/default/v1".to_string()),
        ),
    ]);
    // Hubs that share storage are pointed at it by the caller instead.
    if !shared {
        environment.push((
            "STORAGE_URL".to_string(),
            ComposeValue::Plain("file:///app/storage".to_string()),
        ));
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

#[derive(Debug, Default)]
struct ComposeFile {
    services: Vec<NamedService>,
    volumes: Vec<(String, ComposeVolume)>,
    /// Build secrets, by name; each is the registry token, read from the host's environment.
    secrets: Vec<String>,
}

#[derive(Debug, Default)]
struct ComposeService {
    profiles: Vec<String>,
    build: Option<ComposeBuild>,
    restart: Option<String>,
    image: Option<String>,
    /// A fixed container name, for a container something outside compose has to find by name.
    container_name: Option<String>,
    /// The command replacing the image's `ENTRYPOINT`.
    entrypoint: Vec<String>,
    /// Arguments replacing the image's `CMD`.
    command: Vec<String>,
    /// Container labels, in the order they are written; every value is quoted.
    labels: Vec<(String, String)>,
    healthcheck: Option<ComposeHealthcheck>,
    network_mode: Option<String>,
    scale: Option<u32>,
    ports: Vec<String>,
    env_file: Vec<String>,
    environment: Vec<(String, ComposeValue)>,
    volumes: Vec<String>,
    depends_on: Vec<(String, ComposeDependsOnCondition)>,
    /// The resource limits the service runs under.
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

#[derive(Debug)]
struct ComposeHealthcheck {
    test: Vec<String>,
    interval: String,
    timeout: String,
    retries: u32,
    start_period: String,
}

#[derive(Debug)]
struct ComposeDependsOnCondition {
    condition: String,
    /// Whether compose refuses to start without the dependency; written out only when it does not.
    required: bool,
}

#[derive(Debug, Default)]
struct ComposeVolume;

#[derive(Debug)]
enum ComposeValue {
    Plain(String),
    WrappedDoubleQuoted(Vec<String>),
}

fn render_compose_yaml(compose: &ComposeFile) -> String {
    let mut renderer = ComposeRenderer::default();
    renderer.push_line(0, "services:");
    for (name, service) in &compose.services {
        renderer.render_service(name, service);
    }
    renderer.push_line(0, "volumes:");
    for (name, _) in &compose.volumes {
        renderer.push_line(1, &format!("{name}: {{}}"));
    }
    if !compose.secrets.is_empty() {
        renderer.push_line(0, "secrets:");
        for name in &compose.secrets {
            renderer.push_line(1, &format!("{name}:"));
            renderer.push_line(2, &format!("environment: {REGISTRY_TOKEN_ENV}"));
        }
    }
    renderer.finish()
}

#[derive(Default)]
struct ComposeRenderer {
    output: String,
}

impl ComposeRenderer {
    fn finish(self) -> String {
        self.output
    }

    fn push_line(&mut self, indent: usize, line: &str) {
        self.output.push_str(&"  ".repeat(indent));
        self.output.push_str(line);
        self.output.push('\n');
    }

    /// Write `key:` and then each of `items` as a `- item` line one level deeper, or nothing when there are none.
    ///
    /// An item YAML would read as something other than a string is quoted, since compose wants strings here.
    fn push_list(&mut self, indent: usize, key: &str, items: &[String]) {
        if items.is_empty() {
            return;
        }
        self.push_line(indent, &format!("{key}:"));
        for item in items {
            let rendered = if reads_as_non_string(item) {
                format!("\"{item}\"")
            } else {
                item.clone()
            };
            self.push_line(indent.saturating_add(1), &format!("- {rendered}"));
        }
    }

    fn render_healthcheck(&mut self, healthcheck: &ComposeHealthcheck) {
        self.push_line(2, "healthcheck:");
        self.push_list(3, "test", &healthcheck.test);
        self.push_line(3, &format!("interval: {}", healthcheck.interval));
        self.push_line(3, &format!("timeout: {}", healthcheck.timeout));
        self.push_line(3, &format!("retries: {}", healthcheck.retries));
        self.push_line(3, &format!("start_period: {}", healthcheck.start_period));
    }

    /// Write a service's `build:` key, as the shorthand `build: .` when nothing would sit beneath it.
    ///
    /// A bare `build:` parses as null rather than as compose's defaults of this directory and its Dockerfile.
    fn render_build(&mut self, build: &ComposeBuild) {
        if matches!(build.source, BuildSource::Here) && build.additional_contexts.is_empty() && build.secrets.is_empty()
        {
            self.push_line(2, "build: .");
            return;
        }
        self.push_line(2, "build:");
        if let BuildSource::Within { context, dockerfile } = &build.source {
            self.push_line(3, &format!("context: {context}"));
            self.push_line(3, &format!("dockerfile: {dockerfile}"));
        }
        if !build.additional_contexts.is_empty() {
            self.push_line(3, "additional_contexts:");
            for (name, source) in &build.additional_contexts {
                self.push_line(4, &format!("{name}: {source}"));
            }
        }
        self.push_list(3, "secrets", &build.secrets);
    }

    fn render_service(&mut self, name: &str, service: &ComposeService) {
        self.push_line(1, &format!("{name}:"));
        self.push_list(2, "profiles", &service.profiles);
        if let Some(image) = &service.image {
            self.push_line(2, &format!("image: {image}"));
        }
        if let Some(container_name) = &service.container_name {
            self.push_line(2, &format!("container_name: {container_name}"));
        }
        self.push_list(2, "entrypoint", &service.entrypoint);
        self.push_list(2, "command", &service.command);
        if !service.labels.is_empty() {
            self.push_line(2, "labels:");
            for (key, value) in &service.labels {
                self.push_line(3, &format!("{key}: \"{value}\""));
            }
        }
        if let Some(healthcheck) = &service.healthcheck {
            self.render_healthcheck(healthcheck);
        }
        self.push_list(2, "ports", &service.ports);
        self.push_list(2, "env_file", &service.env_file);
        if let Some(build) = &service.build {
            self.render_build(build);
        }
        if let Some(scale) = service.scale {
            self.push_line(2, &format!("scale: {scale}"));
        }
        if let Some(network_mode) = &service.network_mode {
            self.push_line(2, &format!("network_mode: {network_mode}"));
        }
        if let Some(restart) = &service.restart {
            self.push_line(2, &format!("restart: {restart}"));
        }
        if !service.environment.is_empty() {
            self.push_line(2, "environment:");
            for (key, value) in &service.environment {
                self.render_environment_value(key, value);
            }
        }
        self.push_list(2, "volumes", &service.volumes);
        if !service.depends_on.is_empty() {
            self.push_line(2, "depends_on:");
            for (name, condition) in &service.depends_on {
                self.push_line(3, &format!("{name}:"));
                self.push_line(4, &format!("condition: {}", condition.condition));
                if !condition.required {
                    self.push_line(4, "required: false");
                }
            }
        }
        self.render_limits(&service.limits);
    }

    /// Write a service's resource limits, every default applied, so it runs as the same process does under k3s.
    ///
    /// The CPU weight is written as `cpu_shares` even when it comes from the reservation, because compose outside swarm
    /// ignores `deploy.resources.reservations.cpus`: a container started from it keeps the default weight, where a
    /// Kubernetes container requesting the same CPUs is weighted by that request. The memory reservation, by contrast,
    /// compose does apply, as the cgroup's `memory.low`.
    fn render_limits(&mut self, limits: &ResolvedLimits) {
        let rest = &limits.rest;
        self.push_line(2, "deploy:");
        self.push_line(3, "resources:");
        self.push_line(4, "limits:");
        if let CpuCap::Cpus(cpus) = limits.cpu_cap {
            self.push_line(5, &format!("cpus: \"{cpus}\""));
        }
        self.push_line(5, &format!("memory: {}", compose_bytes(limits.memory)));
        if let Some(pids) = rest.pids {
            self.push_line(5, &format!("pids: {pids}"));
        }
        if let Some(reservation) = limits.memory_low {
            self.push_line(4, "reservations:");
            self.push_line(5, &format!("memory: {}", compose_bytes(reservation)));
        }
        if let Some(weight) = rest.blkio_weight {
            self.push_line(2, "blkio_config:");
            self.push_line(3, &format!("weight: {weight}"));
        }
        let (cpu_quota, cpu_period) = match limits.cpu_cap {
            CpuCap::Quota { quota, period } => (Some(quota.to_string()), Some(period.to_string())),
            CpuCap::Cpus(_) => (None, None),
        };
        let scalars = [
            ("cpu_period", cpu_period),
            ("cpu_quota", cpu_quota),
            ("cpu_shares", Some(limits.cpu_shares.to_string())),
            ("cpuset", rest.cpuset.as_ref().map(|cpuset| format!("\"{cpuset}\""))),
            ("memswap_limit", rest.memswap_limit.map(compose_bytes)),
            ("shm_size", rest.shm_size.map(compose_bytes)),
        ];
        for (key, value) in scalars {
            if let Some(value) = value {
                self.push_line(2, &format!("{key}: {value}"));
            }
        }
        if !rest.ulimits.is_empty() {
            self.push_line(2, "ulimits:");
            for (name, ulimit) in &rest.ulimits {
                let (soft, hard) = ulimit.soft_hard();
                self.push_line(3, &format!("{name}:"));
                self.push_line(4, &format!("soft: {soft}"));
                self.push_line(4, &format!("hard: {hard}"));
            }
        }
    }

    fn render_environment_value(&mut self, key: &str, value: &ComposeValue) {
        match value {
            // Quoted when the value opens with a character YAML reserves as an indicator. A module name carries
            // its owner scope, so it begins with `@` -- which unquoted is a scanner error rather than a string, and
            // produced a compose file nothing could parse.
            //
            // An empty value is quoted for a different reason: written bare it is YAML's null, and compose reads a null
            // entry as "take this from the host environment" rather than as a value. The variable would then be absent
            // from the container and the image's own default would stand -- the opposite of what stating it empty is
            // for.
            //
            // So is one YAML would read as a boolean or a number, which compose rejects as an environment value.
            ComposeValue::Plain(value) => {
                let rendered = if value.is_empty() || value.starts_with(YAML_INDICATORS) || reads_as_non_string(value) {
                    format!("\"{value}\"")
                } else {
                    value.clone()
                };
                self.push_line(3, &format!("{key}: {rendered}"));
            }
            // Wrapped by YAML's own line folding rather than by escaping the breaks away with a trailing `\`. Both keep
            // the value one scalar; folding differs only in leaving a space where the break was, so the list arrives
            // comma-and-space separated -- which is what the `mise` deployment has always written and what the server's
            // per-segment trim expects. The backslash form is banned repo-wide, and this was the one generator still
            // emitting it.
            ComposeValue::WrappedDoubleQuoted(parts) => {
                if let [only] = parts.as_slice() {
                    self.push_line(3, &format!("{key}: \"{only}\""));
                } else if let Some((first, rest)) = parts.split_first() {
                    self.push_line(3, &format!("{key}: \"{first},"));
                    let last_index = rest.len().saturating_sub(1);
                    for (index, part) in rest.iter().enumerate() {
                        let suffix = if index == last_index { "\"" } else { "," };
                        self.push_line(4, &format!("{part}{suffix}"));
                    }
                } else {
                    self.push_line(3, &format!("{key}: \"\""));
                }
            }
        }
    }
}
