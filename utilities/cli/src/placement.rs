//! Where each part of a cluster runs, resolved from its `machines:` once for every deployment format to render.

use std::collections::BTreeMap;

use crate::deployment_types::HUB_TASK;
use crate::error::CliError;
use crate::hub_ws_url::{hub_ws_url, hub_ws_url_at};
use crate::input::{ClusterInput, Machine};
use crate::rfc1123_label;

/// Longest machine name, which is a Kubernetes label value as it is: an RFC 1123 label's 63 characters.
const MACHINE_NAME_MAX: usize = 63;

/// Longest machine address, the length DNS allows a whole host name.
const MACHINE_ADDRESS_MAX: usize = 253;

/// The Kubernetes node label naming the machine a k3s node is, which pins each pod to its machine.
///
/// Prefixed with the organisation so it cannot collide with a label the cluster or another tool already puts on a node.
pub(crate) const MACHINE_LABEL: &str = concat!(et_org::org!(), "/machine");

/// The machine one part of a cluster runs on.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub(crate) enum Site {
    /// The scenario names no machines, so everything shares the host the deployment is started on.
    SingleHost,
    /// A machine running a hub of its own, which everything there reaches at `localhost`.
    ///
    /// The server, which also runs the collector, or another machine `services.hub.machines` lists.
    Local { machine: String },
    /// A machine with no hub, which reaches the server's across the network at the server's address.
    Remote { machine: String, server_address: String },
}

impl Site {
    /// The URL a runner here sets as its `WS_SERVER_URL`: the hub's own host unless the hub is on another machine.
    #[must_use]
    pub(crate) fn hub_ws_url(&self) -> String {
        match self {
            Self::Remote { server_address, .. } => hub_ws_url_at(server_address),
            Self::SingleHost | Self::Local { .. } => hub_ws_url(),
        }
    }

    /// The machine this runs on, as the one-entry list a compose `profiles:` or a dekit `tags:` takes.
    ///
    /// Empty on a single host, which is what keeps those formats' output unchanged for a scenario naming no machines.
    #[must_use]
    pub(crate) fn machine_names(&self) -> Vec<String> {
        match self {
            Self::SingleHost => Vec::new(),
            Self::Local { machine } | Self::Remote { machine, .. } => vec![machine.clone()],
        }
    }

    /// Whether the hub is on another machine, so nothing here can wait on it being ready.
    #[must_use]
    pub(crate) const fn is_remote(&self) -> bool {
        matches!(self, Self::Remote { .. })
    }

    /// The hub a runner here connects to: its own machine's, or the server's when its machine runs none.
    #[must_use]
    pub(crate) fn hub_name(&self, cluster: &ClusterInput) -> String {
        match self {
            Self::Local { machine } => hub_name_on(cluster, machine),
            Self::SingleHost | Self::Remote { .. } => HUB_TASK.to_string(),
        }
    }
}

/// One hub of the cluster and where it runs.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub(crate) struct Hub {
    /// Its mise task, compose service and Kubernetes `Deployment`: [`HUB_TASK`] on the server, suffixed elsewhere.
    pub name: String,
    /// The machine it runs on.
    pub site: Site,
    /// What it reaches the server's services at: `localhost` on the server, the server's address on any other machine.
    pub server_host: String,
}

impl Hub {
    /// Whether this hub runs on the server, beside the collector and what the hubs share.
    #[must_use]
    pub(crate) fn on_server(&self) -> bool {
        self.name == HUB_TASK
    }
}

/// The site of the collector, and of NATS and the object store when the hubs share them: the server, or the one host.
#[must_use]
pub(crate) fn hub_site(cluster: &ClusterInput) -> Site {
    cluster.machines.first().map_or(Site::SingleHost, |server| Site::Local {
        machine: server.name.clone(),
    })
}

/// Every hub the cluster runs, the server's first and the rest in the order `services.hub.machines` lists them.
///
/// One, on the server, unless that list names more.
#[must_use]
pub(crate) fn hubs(cluster: &ClusterInput) -> Vec<Hub> {
    let Some(server) = cluster.machines.first() else {
        return vec![Hub {
            name: HUB_TASK.to_string(),
            site: Site::SingleHost,
            server_host: "localhost".to_string(),
        }];
    };
    let mut machines = vec![server.name.as_str()];
    machines.extend(
        cluster
            .services
            .hub
            .machines
            .iter()
            .map(String::as_str)
            .filter(|machine| *machine != server.name),
    );
    machines
        .into_iter()
        .map(|machine| Hub {
            name: hub_name_on(cluster, machine),
            site: Site::Local {
                machine: machine.to_string(),
            },
            server_host: if machine == server.name {
                "localhost".to_string()
            } else {
                server.address.clone()
            },
        })
        .collect()
}

/// Whether the cluster runs more than one hub, and so the backplane and the object store they share.
#[must_use]
pub(crate) fn shares_hubs(cluster: &ClusterInput) -> bool {
    hubs(cluster).len() > 1
}

/// The name of the hub on `machine`: [`HUB_TASK`] on the server, so a one-hub cluster's names stay what they were.
fn hub_name_on(cluster: &ClusterInput, machine: &str) -> String {
    if cluster.machines.first().is_none_or(|server| server.name == machine) {
        return HUB_TASK.to_string();
    }
    format!("{HUB_TASK}-{machine}")
}

/// The site of `agent`: the machine that lists it, or the server when none does.
#[must_use]
pub(crate) fn agent_site(cluster: &ClusterInput, agent: &str) -> Site {
    let Some(server) = cluster.machines.first() else {
        return Site::SingleHost;
    };
    let machine = cluster
        .machines
        .iter()
        .find(|machine| machine.agents.iter().any(|listed| listed == agent))
        .unwrap_or(server);
    if machine.name == server.name || cluster.services.hub.machines.contains(&machine.name) {
        return Site::Local {
            machine: machine.name.clone(),
        };
    }
    Site::Remote {
        machine: machine.name.clone(),
        server_address: server.address.clone(),
    }
}

/// Reject a `machines:` list that does not describe one placement for every agent and every hub.
///
/// The names and addresses are held to narrow alphabets for the reason `cluster_name` is: both reach the generated
/// README's shell commands, and a name also becomes a Kubernetes label value, a compose profile and a mise task. An
/// address may not open with `-` in particular, since it is handed to `ssh` as its destination, where a leading `-`
/// is read as an option.
pub(crate) fn validate_machines(cluster: &ClusterInput) -> Result<(), CliError> {
    let mut placed: BTreeMap<&str, &str> = BTreeMap::new();
    for (index, machine) in cluster.machines.iter().enumerate() {
        rfc1123_label(&machine.name, MACHINE_NAME_MAX, |reason| CliError::InvalidMachineName {
            name: machine.name.clone(),
            reason,
        })?;
        validate_address(machine)?;
        if cluster
            .machines
            .iter()
            .take(index)
            .any(|earlier| earlier.name == machine.name)
        {
            return Err(CliError::DuplicateMachineName {
                name: machine.name.clone(),
            });
        }
        for agent in &machine.agents {
            if !cluster.agents.iter().any(|declared| declared.name == *agent) {
                return Err(CliError::UnknownMachineAgent {
                    machine: machine.name.clone(),
                    agent: agent.clone(),
                });
            }
            if let Some(first) = placed.insert(agent, &machine.name) {
                return Err(CliError::AgentOnTwoMachines {
                    agent: agent.clone(),
                    first: first.to_string(),
                    second: machine.name.clone(),
                });
            }
        }
    }
    validate_hub_machines(cluster)
}

/// Reject a `services.hub.machines` that does not name each hub's machine once, the server among them.
#[expect(
    clippy::single_call_fn,
    reason = "the hub step of validate_machines, separate for readability"
)]
fn validate_hub_machines(cluster: &ClusterInput) -> Result<(), CliError> {
    let listed = &cluster.services.hub.machines;
    let Some(server) = cluster.machines.first() else {
        return listed.first().map_or(Ok(()), |machine| {
            Err(CliError::HubMachinesWithoutMachines {
                machine: machine.clone(),
            })
        });
    };
    for (index, machine) in listed.iter().enumerate() {
        if !cluster.machines.iter().any(|declared| declared.name == *machine) {
            return Err(CliError::UnknownHubMachine {
                machine: machine.clone(),
            });
        }
        if listed.iter().take(index).any(|earlier| earlier == machine) {
            return Err(CliError::DuplicateHubMachine {
                machine: machine.clone(),
            });
        }
    }
    if !listed.is_empty() && !listed.contains(&server.name) {
        return Err(CliError::HubMachinesWithoutServer {
            server: server.name.clone(),
        });
    }
    Ok(())
}

/// Check `machine`'s address is a lowercase host name or IPv4 address, naming why it is not.
fn validate_address(machine: &Machine) -> Result<(), CliError> {
    let address = &machine.address;
    let reason = if address.is_empty() {
        "it is empty".to_owned()
    } else if address.len() > MACHINE_ADDRESS_MAX {
        format!("it is longer than {MACHINE_ADDRESS_MAX} characters")
    } else if let Some(bad) = address
        .chars()
        .find(|found| !(found.is_ascii_lowercase() || found.is_ascii_digit() || matches!(found, '-' | '.')))
    {
        format!("it contains {bad:?}")
    } else if !address.starts_with(|first: char| first.is_ascii_alphanumeric()) {
        "it does not start with a letter or digit".to_owned()
    } else {
        return Ok(());
    };
    Err(CliError::InvalidMachineAddress {
        machine: machine.name.clone(),
        address: address.clone(),
        reason,
    })
}
