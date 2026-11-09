//! The README a generated deployment carries: what the directory holds, then how to run each format it was written in.
//!
//! What it says is the templates' under `templates/readme`; this module works out the facts each paragraph turns on.

use std::collections::BTreeSet;
use std::path::Path;

use fs_err as fs;
use serde::Serialize;

use crate::deployment_types::{self, ImageNeeds};
use crate::error::CliError;
use crate::expose::{self, ExposedAddress};
use crate::input::{self, ArtifactSource, Board, BoardSpec, ClusterInput, Delivery, OutputType};
use crate::templates::render;
use crate::{
    IMAGE_REGISTRY, SECRETS_ENV_FILE, cluster_module_names, hub_ws_url_at, limits, module_path_contexts, placement,
};

/// What the README is rendered from.
#[derive(Serialize)]
struct Readme<'cluster> {
    name: &'cluster str,
    /// The files the deployment wrote, in the order the README lists them.
    output_files: Vec<&'static str>,
    module_names: Vec<String>,
    published: bool,
    shares_hubs: bool,
    /// Whether a format runs the `mise.toml` tasks, which need what they name installed before the first run.
    installs: bool,
    sections: Vec<Section<'cluster>>,
    /// The sections loading each board among the machines.
    boards: Vec<Launch<'cluster>>,
    registry: &'static str,
    secrets_file: &'static str,
    token_env: &'static str,
    token_secret: &'static str,
}

/// The README section running one format, with the subsections that format adds.
#[derive(Serialize)]
struct Section<'cluster> {
    /// The format's own name, as `--output-type` takes it.
    format: &'static str,
    /// What the format's headings call it.
    ///
    /// Every format's section lands in the one README, and markdownlint rejects a repeated heading.
    heading: &'static str,
    run: Launch<'cluster>,
    /// Every machine the scenario names, the server first; none for a scenario on a single host.
    machines: Vec<MachineShare<'cluster>>,
    /// Whether any machine's address is loopback, which plays every machine on one host.
    loopback: bool,
    /// The processes whose limits the format does not apply.
    unapplied_limits: Vec<&'cluster str>,
    /// Whether the format runs scheduled agents in a way the operator starting it has to be told about.
    scheduled: bool,
}

/// How a section runs its format, which picks the template under `templates/readme` that says so.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum Launch<'cluster> {
    /// The server is a board running the hub, which leaves the format only the runners on the other machines.
    BoardHub {
        server: &'cluster str,
        title: &'static str,
    },
    Mise,
    Dekit,
    ProcessCompose,
    DockerCompose {
        /// The machines whose profile starts on one host.
        profiles: Vec<&'cluster str>,
        runner_addresses: Vec<ExposedAddress>,
    },
    K3s(K3s<'cluster>),
    /// A board running the hub: the whole scenario as a `firmware-*` format, or the server of one spanning machines.
    Firmware(Firmware),
    /// A board running only the runners placed on it, which reach the server's hub across the network.
    RunnerBoard {
        machine: &'cluster str,
        title: &'static str,
        runners: &'static [&'static str],
        hub_url: String,
        network_setup: &'static str,
        load_preparation: &'static str,
        flash: String,
    },
}

/// What the k3s section says about producing the images, loading the credential and reaching the cluster.
#[derive(Serialize)]
struct K3s<'cluster> {
    /// Whether the scenario image builds from the repository root rather than from this directory.
    repository: bool,
    /// Whether the build fetches released modules, and so needs the registry token.
    registry_token: bool,
    /// The scenario Dockerfile, relative to where the build runs.
    dockerfile: String,
    /// The named build context each `module_paths:` entry is handed to the build as, each `name=path`.
    build_contexts: Vec<String>,
    /// The runner kinds whose images the manifests name, in the order the input first names them.
    runners: Vec<String>,
    /// Whether the manifests name the recovery supervisor's image too.
    recovery: bool,
    /// The container runtime plugins every node has to be readied for.
    plugins: Vec<&'static str>,
    containerd_template_file: &'static str,
    containerd_template_path: &'static str,
    addresses: Vec<ExposedAddress>,
    /// Whether the hub serves a page nothing outside the cluster can reach.
    hidden_page: bool,
    /// The address of each machine the scenario image is imported onto.
    hub_imports: Vec<&'cluster str>,
    /// Every other image a machine runs, imported onto it.
    image_imports: Vec<ImageImport<'cluster>>,
}

/// A board's firmware running the hub, and how it is built and loaded.
#[derive(Serialize)]
struct Firmware {
    heading: String,
    runners: &'static [&'static str],
    flash_size: &'static str,
    network_setup: &'static str,
    load_preparation: &'static str,
    /// The command that builds the firmware and loads it.
    flash: String,
    /// The command that serves the same modules from this host instead.
    host_test: String,
    /// Where the board redirects a package too big for its flash.
    elsewhere: &'static str,
}

/// One machine's share of the scenario, and the names each format starts it by.
#[derive(Serialize)]
struct MachineShare<'cluster> {
    name: &'cluster str,
    address: &'cluster str,
    /// The board the machine is, by what a person calls it; a machine running its share as the format's tasks is none.
    board: Option<&'static str>,
    /// The mise task starting the machine's share.
    task: String,
    /// The mise task loading the machine's board, which is its whole share when it is one.
    flash: String,
    /// The `--node-label` values the machine joins a k3s cluster with.
    labels: Vec<String>,
}

/// An image a machine runs, other than the scenario image.
#[derive(Serialize)]
struct ImageImport<'cluster> {
    image: String,
    address: &'cluster str,
}

/// How the README's `docker build` of the scenario image runs: what it needs, and the paths it names.
struct ImageBuild {
    needs: ImageNeeds,
    /// The scenario Dockerfile, relative to where the build runs.
    dockerfile: String,
    build_contexts: Vec<String>,
}

/// Write the README for a deployment of `output_types`: what the directory holds and how to run each format.
pub(crate) fn write_readme(
    cluster: &ClusterInput,
    output_dir: &Path,
    output_types: &[OutputType],
) -> Result<(), CliError> {
    let needs = deployment_types::scenario_image_needs(cluster)?;
    let (dockerfile, context_root) = if needs.repository {
        (
            scenario_dockerfile_path(output_dir),
            edge_toolkit::config::get_project_root(),
        )
    } else {
        ("Dockerfile".to_string(), std::path::absolute(output_dir)?)
    };
    // Relative to where the README's `docker build` runs from.
    let build_contexts = module_path_contexts(&cluster.module_paths, &context_root)
        .into_iter()
        .map(|(name, path)| format!("{name}={path}"))
        .collect();
    let image = ImageBuild {
        needs,
        dockerfile,
        build_contexts,
    };
    let readme = Readme {
        boards: boards(cluster),
        installs: output_types.iter().any(|output_type| output_type.runs_mise_tasks()),
        module_names: cluster_module_names(cluster),
        name: &cluster.cluster_name,
        output_files: generated_output_files(cluster, output_types),
        published: matches!(cluster.artifact_source, ArtifactSource::Published),
        registry: IMAGE_REGISTRY,
        secrets_file: SECRETS_ENV_FILE,
        sections: output_types
            .iter()
            .map(|output_type| section(cluster, *output_type, &image))
            .collect(),
        shares_hubs: placement::shares_hubs(cluster),
        token_env: deployment_types::REGISTRY_TOKEN_ENV,
        token_secret: deployment_types::REGISTRY_TOKEN_SECRET,
    };
    fs::write(output_dir.join("README.md"), render("readme.md.j2", &readme)?)?;
    Ok(())
}

/// The path the generated README tells a reader to build this scenario's image from.
///
/// Only the parent is rendered, so the command keeps naming the last segment through the `scenario` shell variable it
/// already sets rather than repeating the name. Joined from the path's own components rather than displayed, because
/// the result is committed: a `Display` of the same path writes `\` on Windows and `/` everywhere else, which would
/// make the file drift by platform and fail the check that holds it stable. Regeneration passes a repository-relative
/// path, which is what the command needs, since it runs from the repository root.
///
/// A single-component output directory has a parent, and it is the empty path rather than `None` -- so this cannot lean
/// on `unwrap_or` and has to check the rendered parent. Prefixing an empty one would produce `/$scenario/Dockerfile`,
/// an absolute path to a directory nobody has.
#[must_use]
pub fn scenario_dockerfile_path(output_dir: &Path) -> String {
    let parent = output_dir
        .parent()
        .unwrap_or(output_dir)
        .iter()
        .map(|component| component.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    if parent.is_empty() {
        return "$scenario/Dockerfile".to_string();
    }
    format!("{parent}/$scenario/Dockerfile")
}

/// The files a deployment of `output_types` writes for `cluster`, in the order the README lists them.
fn generated_output_files(cluster: &ClusterInput, output_types: &[OutputType]) -> Vec<&'static str> {
    let mut files = Vec::new();
    for output_type in output_types {
        // `dekit.yaml` and `process-compose.yaml` run the tasks of `mise.toml`, which is written with either whether or
        // not it was asked for -- and listed once, however many formats wrote it.
        if matches!(output_type, OutputType::Dekit | OutputType::ProcessCompose) {
            files.push(OutputType::Mise.output_file_name());
        }
        files.push(output_type.output_file_name());
        if matches!(output_type, OutputType::K3s) && deployment_types::needs_containerd_template(cluster) {
            files.push(deployment_types::CONTAINERD_TEMPLATE_FILE);
        }
    }
    let mut listed = BTreeSet::new();
    files.retain(|file| listed.insert(*file));
    files
}

/// The section running `output_type`.
fn section<'cluster>(
    cluster: &'cluster ClusterInput,
    output_type: OutputType,
    image: &ImageBuild,
) -> Section<'cluster> {
    let scheduled = cluster.agents.iter().any(|agent| !agent.schedule.is_unset());
    Section {
        format: output_type.name(),
        heading: heading(output_type),
        loopback: cluster
            .machines
            .iter()
            .any(|machine| machine.address == "localhost" || machine.address.starts_with("127.")),
        machines: machine_shares(cluster),
        run: run(cluster, output_type, image),
        // Compose is the one format whose schedule differs visibly from a `CronJob`, and the one that asks for a
        // privilege.
        scheduled: scheduled && matches!(output_type, OutputType::DockerCompose),
        unapplied_limits: unapplied_limits(cluster, output_type),
    }
}

/// What `output_type`'s headings call it.
const fn heading(output_type: OutputType) -> &'static str {
    match output_type {
        OutputType::Mise => "Mise",
        OutputType::Dekit => "dekit",
        OutputType::ProcessCompose => "process-compose",
        OutputType::DockerCompose => "Docker Compose",
        OutputType::K3s => "k3s",
        OutputType::FirmwarePico2 => Board::Pico2.spec().title,
        OutputType::FirmwareEsp32c6 => Board::Esp32c6.spec().title,
        OutputType::FirmwarePicoPlus2 => Board::PicoPlus2.spec().title,
    }
}

/// The processes whose limits `output_type` does not apply; none for a format that applies them.
///
/// Named in the README rather than left out of the output silently, so an operator relying on a limit learns which
/// deployments do not hold it. Only the formats running each process on the host leave limits unapplied: compose and
/// k3s enforce them, and a board refuses a runner that sets any, since it has no way to hold one to them.
fn unapplied_limits(cluster: &ClusterInput, output_type: OutputType) -> Vec<&str> {
    if output_type.runs_mise_tasks() {
        return limits::limited_processes(cluster);
    }
    Vec::new()
}

/// How the section of `output_type` runs it.
fn run<'cluster>(cluster: &'cluster ClusterInput, output_type: OutputType, image: &ImageBuild) -> Launch<'cluster> {
    if let (Delivery::Firmware(board), Some(server)) = (placement::server_delivery(cluster), cluster.machines.first()) {
        return Launch::BoardHub {
            server: &server.name,
            title: board.spec().title,
        };
    }
    match output_type {
        OutputType::Mise => Launch::Mise,
        OutputType::Dekit => Launch::Dekit,
        OutputType::ProcessCompose => Launch::ProcessCompose,
        OutputType::DockerCompose => Launch::DockerCompose {
            profiles: compose_single_host_profiles(cluster),
            runner_addresses: expose::addresses(cluster, false),
        },
        OutputType::K3s => Launch::K3s(k3s(cluster, image)),
        OutputType::FirmwarePico2 => firmware_output(Board::Pico2.spec()),
        OutputType::FirmwareEsp32c6 => firmware_output(Board::Esp32c6.spec()),
        OutputType::FirmwarePicoPlus2 => firmware_output(Board::PicoPlus2.spec()),
    }
}

/// The machines whose `--profile` starts the scenario on one host, or none for a scenario naming no machines.
///
/// Every machine's profile, but that of a machine running a hub besides the server: every hub listens on the same
/// ports, so on one host the server's is the only one that can start, and each runner reaches it at `localhost`.
fn compose_single_host_profiles(cluster: &ClusterInput) -> Vec<&str> {
    let hubs = placement::hubs(cluster);
    cluster
        .machines
        .iter()
        .filter(|machine| {
            !hubs
                .iter()
                .any(|hub| !hub.on_server() && hub.site.machine_names().contains(&machine.name))
        })
        .map(|machine| machine.name.as_str())
        .collect()
}

/// Every machine's share of the scenario, the server first.
fn machine_shares(cluster: &ClusterInput) -> Vec<MachineShare<'_>> {
    cluster
        .machines
        .iter()
        .map(|machine| MachineShare {
            address: &machine.address,
            board: match machine.device.delivery() {
                Delivery::Firmware(board) => Some(board.spec().title),
                Delivery::Tasks => None,
            },
            flash: deployment_types::firmware_flash_task(&machine.name),
            labels: k3s_node_labels(cluster, &machine.name),
            name: &machine.name,
            task: deployment_types::machine_task(&machine.name),
        })
        .collect()
}

/// The `--node-label` values `machine` joins the k3s cluster with.
///
/// The machine label the manifests pin pods with, and for a scenario that exposes a port, the load-balancer pool that
/// keeps each exposed `Service` to the machine it names.
fn k3s_node_labels(cluster: &ClusterInput, machine: &str) -> Vec<String> {
    let mut labels = vec![format!("{}={machine}", placement::MACHINE_LABEL)];
    if expose::any_exposed(cluster) {
        labels.push(format!("{}={machine}", deployment_types::K3S_LB_POOL_LABEL));
    }
    labels
}

/// The k3s section's facts, `image` being how the scenario image builds.
fn k3s<'cluster>(cluster: &'cluster ClusterInput, image: &ImageBuild) -> K3s<'cluster> {
    let (hub_imports, image_imports) = k3s_machine_imports(cluster);
    K3s {
        addresses: expose::addresses(cluster, true),
        build_contexts: image.build_contexts.clone(),
        containerd_template_file: deployment_types::CONTAINERD_TEMPLATE_FILE,
        containerd_template_path: deployment_types::CONTAINERD_TEMPLATE_PATH,
        dockerfile: image.dockerfile.clone(),
        hidden_page: deployment_types::serves_a_page(cluster) && cluster.services.hub.expose.is_empty(),
        hub_imports,
        image_imports,
        plugins: deployment_types::k3s_plugin_names(cluster),
        recovery: !cluster.recovery.is_unset(),
        registry_token: image.needs.registry_token,
        repository: image.needs.repository,
        runners: runner_kinds_where(cluster, |_agent| true),
    }
}

/// The image imports of a k3s scenario spread across machines, none for one on a single host.
///
/// The scenario image goes to each machine running a hub, and each locally built image to each machine running it: the
/// recovery supervisor to the server, and a runner image to each machine with a runner of that kind. A published
/// scenario's are pulled by the cluster, so only the scenario image is imported for it.
fn k3s_machine_imports(cluster: &ClusterInput) -> (Vec<&str>, Vec<ImageImport<'_>>) {
    let Some(server) = cluster.machines.first() else {
        return (Vec::new(), Vec::new());
    };
    let mut hub_imports = Vec::new();
    for hub in placement::hubs(cluster) {
        hub_imports.extend(
            cluster
                .machines
                .iter()
                .filter(|machine| hub.site.machine_names().contains(&machine.name))
                .map(|machine| machine.address.as_str()),
        );
    }
    let mut image_imports = Vec::new();
    if matches!(cluster.artifact_source, ArtifactSource::Local) {
        if !cluster.recovery.is_unset() {
            image_imports.push(ImageImport {
                address: &server.address,
                image: "et-recovery-supervisor:latest".to_owned(),
            });
        }
        for machine in &cluster.machines {
            for kind in machine_runner_kinds(cluster, &machine.name) {
                image_imports.push(ImageImport {
                    address: &machine.address,
                    image: format!("et-ws-{kind}-runner:latest"),
                });
            }
        }
    }
    (hub_imports, image_imports)
}

/// The distinct runner kinds of the agents placed on `machine`, in the order they first appear.
fn machine_runner_kinds(cluster: &ClusterInput, machine: &str) -> Vec<String> {
    runner_kinds_where(cluster, |agent| {
        placement::agent_site(cluster, &agent.name)
            .machine_names()
            .iter()
            .any(|name| name == machine)
    })
}

/// The distinct runner kinds of the agents `include` accepts, in the order they first appear.
///
/// Deduplicated because two agents on the same runner need its image built once, and ordered by first appearance
/// rather than sorted so the generated instructions read in the order the input declares.
fn runner_kinds_where<F>(cluster: &ClusterInput, include: F) -> Vec<String>
where
    F: Fn(&input::Agent) -> bool,
{
    let mut kinds: Vec<String> = Vec::new();
    for agent in cluster.agents.iter().filter(|agent| include(agent)) {
        let Some(kind) = agent.runner.as_deref().map(str::trim).filter(|kind| !kind.is_empty()) else {
            continue;
        };
        if !kinds.iter().any(|seen| seen == kind) {
            kinds.push(kind.to_string());
        }
    }
    kinds
}

/// The section of a `firmware-*` format, which puts the whole scenario on `board`.
fn firmware_output(board: BoardSpec) -> Launch<'static> {
    let env = board.output_type.name();
    Launch::Firmware(firmware(
        board,
        format!("Run As {} Firmware", board.title),
        format!("MISE_ENV={env} mise run {}", deployment_types::FIRMWARE_FLASH_TASK),
        format!("MISE_ENV={env} mise run {}", deployment_types::FIRMWARE_HOST_TEST_TASK),
    ))
}

/// `board`'s firmware running the hub, under `heading`, loaded by `flash` and served from here by `host_test`.
const fn firmware(board: BoardSpec, heading: String, flash: String, host_test: String) -> Firmware {
    Firmware {
        elsewhere: deployment_types::FIRMWARE_ELSEWHERE_URL,
        flash,
        flash_size: board.flash,
        heading,
        host_test,
        load_preparation: board.load_preparation,
        network_setup: board.network_setup,
        runners: board.runners,
    }
}

/// The sections loading each board among the machines; none when no machine is a board.
///
/// That is the hub onto the server, when it is a board, and the runners onto every other board.
fn boards(cluster: &ClusterInput) -> Vec<Launch<'_>> {
    let Some(server) = cluster.machines.first() else {
        return Vec::new();
    };
    let mut boards = Vec::new();
    for machine in &cluster.machines {
        let Delivery::Firmware(board) = machine.device.delivery() else {
            continue;
        };
        let board = board.spec();
        let flash = format!("mise run {}", deployment_types::firmware_flash_task(&machine.name));
        boards.push(if machine.name == server.name {
            let host_test = format!(
                "mise run {}-{}",
                deployment_types::FIRMWARE_HOST_TEST_TASK,
                machine.name
            );
            Launch::Firmware(firmware(
                board,
                format!("Load The Hub Onto `{}`", machine.name),
                flash,
                host_test,
            ))
        } else {
            Launch::RunnerBoard {
                flash,
                hub_url: hub_ws_url_at(&server.address),
                load_preparation: board.load_preparation,
                machine: &machine.name,
                network_setup: board.network_setup,
                runners: board.runners,
                title: board.title,
            }
        });
    }
    boards
}
