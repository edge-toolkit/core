use std::path::Path;

use edge_toolkit::ports::Services;
use fs_err as fs;
use serde_yaml::{Mapping, Value};

use super::mise::collector_container;
use super::yaml::{document, insert};
use super::{COLLECTOR_TASK, HUB_TASK, OPENER_TASK};
use crate::error::CliError;
use crate::input::ClusterInput;
use crate::placement::{Site, hub_site};
use crate::schedule::Schedule;
use crate::{OutputType, scenario_runners};

/// The mise tool that installs process-compose, which the `mise.toml` written alongside declares.
pub(crate) const PROCESS_COMPOSE_TOOL: &str = "aqua:F1bonacc1/process-compose";

/// The process-compose release the generated file is written for, pinned so its `schedule:` semantics cannot drift.
pub(crate) const PROCESS_COMPOSE_VERSION: &str = "1.122.0";

/// The config schema version process-compose reads the file as.
const SCHEMA_VERSION: &str = "0.5";

/// SIGINT, the interrupt a mise task is written to stop on, as process-compose's numeric `signal:` takes it.
const SIGINT: u8 = 2;

/// How long the collector gets to exit once stopped, matching the dekit deployment's allowance for `docker stop`.
const COLLECTOR_STOP_TIMEOUT_SECONDS: u16 = 30;

/// Seconds between readiness checks.
const READINESS_PERIOD_SECONDS: u16 = 2;

/// Readiness checks a process may fail before it is given up on: an hour at [`READINESS_PERIOD_SECONDS`].
///
/// Exhausting them is fatal rather than a warning -- process-compose stops the process -- and a local hub compiles
/// itself with `cargo run` before it can answer, which on a cold checkout takes many minutes. An hour is a bound on a
/// process that is never going to come up, not an estimate of one that is.
const READINESS_FAILURE_THRESHOLD: u16 = 1800;

/// Write the `process-compose.yaml` that runs the scenario with `process-compose up`, one process per mise task.
///
/// Like `dekit.yaml`, each process runs a task of the `mise.toml` written alongside rather than a copy of its command
/// line, so the two cannot describe different deployments, and like it each process waits on what it talks to being
/// ready. What this format adds over dekit is a scheduler: a scheduled agent's runner task is started on each tick of
/// its `schedule:` and runs its module once, and a tick that finds the previous run still going is skipped.
///
/// A multi-machine cluster puts each process in the namespace of its machine, so `-n <machine>` starts one machine's
/// share.
pub fn generate_process_compose_deployment(cluster: &ClusterInput, output_dir: &Path) -> Result<(), CliError> {
    let runners = scenario_runners(cluster)?;
    let server = hub_site(cluster);

    let mut processes = Mapping::new();
    let mut collector = process(COLLECTOR_TASK, &[], &server);
    insert(
        &mut collector,
        "readiness_probe",
        http_ready(Services::OtlpCollector.port(), "/healthz"),
    );
    // `docker run` is stopped through Docker rather than by signalling the client, for the reason dekit's stop gives.
    if let Some(Value::Mapping(shutdown)) = collector.get_mut("shutdown") {
        let container = collector_container(&cluster.cluster_name);
        insert(shutdown, "command", Value::from(format!("docker stop {container}")));
        insert(shutdown, "timeout_seconds", Value::from(COLLECTOR_STOP_TIMEOUT_SECONDS));
    }
    insert(&mut processes, COLLECTOR_TASK, Value::Mapping(collector));

    let mut hub = process(HUB_TASK, &[COLLECTOR_TASK], &server);
    insert(
        &mut hub,
        "readiness_probe",
        http_ready(Services::InsecureWebSocketServer.port(), "/health"),
    );
    insert(&mut processes, HUB_TASK, Value::Mapping(hub));

    for runner in runners {
        // A runner on another machine cannot wait on a hub process this one does not hold, so it restarts until the hub
        // on the server answers instead. A scheduled one is not restarted: its next tick is its retry.
        let remote = runner.site.is_remote();
        let deps: &[&str] = if remote { &[] } else { &[HUB_TASK] };
        let mut runner_process = process(&runner.name, deps, &runner.site);
        if runner.schedule.is_unset() {
            if remote {
                insert(&mut runner_process, "availability", restart("on_failure"));
            }
        } else {
            insert(&mut runner_process, "schedule", schedule(&runner.schedule));
        }
        insert(&mut processes, &runner.name, Value::Mapping(runner_process));
    }

    // Disabled, since it opens the browser and exits: it is there to be started once the collector is up, which its
    // dependency then waits for.
    let mut opener = process(OPENER_TASK, &[COLLECTOR_TASK], &server);
    insert(&mut opener, "disabled", Value::Bool(true));
    insert(&mut processes, OPENER_TASK, Value::Mapping(opener));

    let mut config = Mapping::new();
    insert(&mut config, "version", Value::from(SCHEMA_VERSION));
    insert(&mut config, "processes", Value::Mapping(processes));
    fs::write(
        output_dir.join(OutputType::ProcessCompose.output_file_name()),
        document(&config)?,
    )?;
    Ok(())
}

/// The process that runs the mise task `name` once each of `deps` reports healthy, in the namespace of `site`.
///
/// Stopped with SIGINT rather than process-compose's SIGTERM, for the reason the dekit deployment gives.
fn process(name: &str, deps: &[&str], site: &Site) -> Mapping {
    let mut process = Mapping::new();
    insert(&mut process, "command", Value::from(format!("mise run {name}")));
    if !deps.is_empty() {
        let mut depends_on = Mapping::new();
        for dep in deps {
            let mut condition = Mapping::new();
            insert(&mut condition, "condition", Value::from("process_healthy"));
            insert(&mut depends_on, dep, Value::Mapping(condition));
        }
        insert(&mut process, "depends_on", Value::Mapping(depends_on));
    }
    if let [machine] = site.machine_names().as_slice() {
        insert(&mut process, "namespace", Value::from(machine.as_str()));
    }
    let mut shutdown = Mapping::new();
    insert(&mut shutdown, "signal", Value::from(SIGINT));
    insert(&mut process, "shutdown", Value::Mapping(shutdown));
    process
}

/// A readiness probe that passes once `path` on `port` of this host answers a GET with a 2xx status.
///
/// The port goes in `port`, never `num_port`: process-compose recomputes `num_port` from `port` and zeroes it when
/// `port` is unset, so a probe written with `num_port` alone checks port 80 and fails without saying why.
fn http_ready(port: u16, path: &str) -> Value {
    let mut http_get = Mapping::new();
    insert(&mut http_get, "host", Value::from("localhost"));
    insert(&mut http_get, "scheme", Value::from("http"));
    insert(&mut http_get, "path", Value::from(path));
    insert(&mut http_get, "port", Value::from(port));
    let mut probe = Mapping::new();
    insert(&mut probe, "http_get", Value::Mapping(http_get));
    insert(&mut probe, "period_seconds", Value::from(READINESS_PERIOD_SECONDS));
    insert(
        &mut probe,
        "failure_threshold",
        Value::from(READINESS_FAILURE_THRESHOLD),
    );
    Value::Mapping(probe)
}

/// An `availability:` block with the restart policy `policy`.
fn restart(policy: &str) -> Value {
    let mut availability = Mapping::new();
    insert(&mut availability, "restart", Value::from(policy));
    Value::Mapping(availability)
}

/// A `schedule:` block, read in the host's zone unless the scenario names one.
fn schedule(schedule: &Schedule) -> Value {
    let mut block = Mapping::new();
    insert(&mut block, "cron", Value::from(schedule.cron.trim()));
    if !schedule.time_zone.is_empty() {
        insert(&mut block, "timezone", Value::from(schedule.time_zone.as_str()));
    }
    Value::Mapping(block)
}
