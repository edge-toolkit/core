use std::path::Path;

use edge_toolkit::ports::Services;
use fs_err as fs;
use serde::Serialize;
use serde_yaml::{Mapping, Value};

use super::document;
use super::mise::{COLLECTOR_TASK, HUB_TASK, OPENER_TASK, collector_container};
use super::yaml::{insert, strings};
use crate::error::CliError;
use crate::input::ClusterInput;
use crate::placement::{Site, hub_site};
use crate::{OutputType, hub_http_base, scenario_runners};

/// How long the collector gets to exit once stopped.
///
/// `docker stop` itself waits up to ten seconds before it kills the container, which is dekit's whole default, so the
/// default would kill the task while Docker was still stopping it.
const COLLECTOR_STOP_TIMEOUT: &str = "30s";

/// Write the `dekit.yaml` that runs the scenario with `dekit up`, one task per process.
///
/// Each task is a task of the generated `mise.toml` rather than a copy of its command line, so the two formats cannot
/// describe different deployments: this adds only what dekit knows how to do and mise does not. That is also why the
/// `mise.toml` is always written alongside -- this file is a view onto it, not a deployment of its own.
///
/// What it adds is order. mise starts the hub, the collector and the runners together, so a runner can ask the hub for
/// its module before the hub is listening. Here each waits on what it talks to being ready, the same edges the compose
/// stack gates on: the hub on the collector's health endpoint, every runner on the hub's. The task names need no path
/// of their own, because the file is its own project root: dekit takes the nearest `dekit.yaml` as the root, and
/// starts every task there, which is where mise finds the `mise.toml`.
pub fn generate_dekit_deployment(cluster: &ClusterInput, output_dir: &Path) -> Result<(), CliError> {
    let runners = scenario_runners(cluster)?;
    let collector_url = format!("http://localhost:{}", Services::OtlpCollector.port());
    // A multi-machine cluster tags each task with its machine and starts none on `dekit up`, since every machine runs
    // the same file and `up` would otherwise start the whole cluster on each of them; `dekit start +<machine>` starts
    // one machine's share instead.
    let autostart = cluster.machines.is_empty();
    let server = hub_site(cluster);

    let mut tasks = Mapping::new();
    let mut collector = task(COLLECTOR_TASK, &[], autostart, &server);
    insert(&mut collector, "ready", http_ready(&format!("{collector_url}/healthz")));
    // `docker run` is stopped through Docker rather than by signalling the client: a client that dies before it has
    // passed the signal on leaves the container running and holding the collector's port.
    let mut stop = Mapping::new();
    let container = collector_container(&cluster.cluster_name);
    insert(&mut stop, "cmd", strings(&["docker", "stop", &container]));
    insert(&mut stop, "timeout", Value::from(COLLECTOR_STOP_TIMEOUT));
    insert(&mut collector, "stop", Value::Mapping(stop));
    insert(&mut tasks, COLLECTOR_TASK, Value::Mapping(collector));

    let mut hub = task(HUB_TASK, &[COLLECTOR_TASK], autostart, &server);
    insert(&mut hub, "ready", http_ready(&format!("{}/health", hub_http_base())));
    insert(&mut tasks, HUB_TASK, Value::Mapping(hub));

    for runner in runners {
        // A runner on another machine cannot wait on a hub task this runner does not hold, so it restarts until the
        // hub on the server answers instead.
        let remote = runner.site.is_remote();
        let deps: &[&str] = if remote { &[] } else { &[HUB_TASK] };
        let mut runner_task = task(&runner.name, deps, autostart, &runner.site);
        if remote {
            insert(&mut runner_task, "autorestart", Value::from("on-failure"));
        }
        insert(&mut tasks, &runner.name, Value::Mapping(runner_task));
    }

    // A job, since it opens the browser and exits, and not started on `dekit up`: it is there to be run once the
    // collector is up, which its dependency then waits for.
    let mut opener = Mapping::new();
    insert(&mut opener, "type", Value::from("job"));
    opener.extend(task(OPENER_TASK, &[COLLECTOR_TASK], false, &server));
    insert(&mut tasks, OPENER_TASK, Value::Mapping(opener));

    let config = DekitConfig {
        // SIGINT rather than dekit's SIGTERM, because it is the interrupt a mise task is written to stop on -- the same
        // one Ctrl-C sends the whole process group when the task runs in a terminal.
        defaults: DekitDefaults { stop: "SIGINT" },
        tasks,
    };
    fs::write(
        output_dir.join(OutputType::Dekit.output_file_name()),
        document(&config)?,
    )?;
    Ok(())
}

/// The task that runs the mise task `name`, after `deps`, started by `dekit up` when `autostart` is set.
///
/// A task left out of `autostart` still starts when another that depends on it does, so only what nothing else pulls in
/// strictly needs it. Every long-running process is marked anyway, because which of them a scenario's runners pull in
/// varies, and one with no runners would otherwise start nothing at all.
///
/// Tagged with the machine at `site`, so `dekit start +<machine>` selects it; a single-host task carries no tags.
fn task(name: &str, deps: &[&str], autostart: bool, site: &Site) -> Mapping {
    let mut task = Mapping::new();
    insert(&mut task, "cmd", strings(&["mise", "run", name]));
    if !deps.is_empty() {
        insert(&mut task, "deps", strings(deps));
    }
    let tags = site.machine_names();
    if !tags.is_empty() {
        insert(
            &mut task,
            "tags",
            Value::Sequence(tags.into_iter().map(Value::from).collect()),
        );
    }
    if autostart {
        insert(&mut task, "autostart", Value::Bool(true));
    }
    task
}

/// A `ready` check that passes once `url` answers a GET with a 2xx or 3xx status.
fn http_ready(url: &str) -> Value {
    let mut ready = Mapping::new();
    insert(&mut ready, "http", Value::from(url));
    Value::Mapping(ready)
}

/// The whole of a `dekit.yaml`: settings every task shares, then the tasks in the order the TUI lists them.
#[derive(Debug, Serialize)]
struct DekitConfig {
    defaults: DekitDefaults,
    tasks: Mapping,
}

/// Settings every task takes unless it sets its own.
#[derive(Debug, Serialize)]
struct DekitDefaults {
    stop: &'static str,
}
