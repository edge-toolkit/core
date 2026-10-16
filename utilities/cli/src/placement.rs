//! Where each part of a cluster runs, resolved from its `machines:` once for every deployment format to render.

use std::collections::BTreeMap;

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
    /// The server machine, the first listed, which also runs the hub and the collector.
    Server { machine: String },
    /// Any other machine, which reaches the hub across the network at the server's address.
    Remote { machine: String, server_address: String },
}

impl Site {
    /// The URL a runner here sets as its `WS_SERVER_URL`: the hub's own host unless the hub is on another machine.
    #[must_use]
    pub(crate) fn hub_ws_url(&self) -> String {
        match self {
            Self::Remote { server_address, .. } => hub_ws_url_at(server_address),
            Self::SingleHost | Self::Server { .. } => hub_ws_url(),
        }
    }

    /// The machine this runs on, as the one-entry list a compose `profiles:` or a dekit `tags:` takes.
    ///
    /// Empty on a single host, which is what keeps those formats' output unchanged for a scenario naming no machines.
    #[must_use]
    pub(crate) fn machine_names(&self) -> Vec<String> {
        match self {
            Self::SingleHost => Vec::new(),
            Self::Server { machine } | Self::Remote { machine, .. } => vec![machine.clone()],
        }
    }

    /// Whether the hub is on another machine, so nothing here can wait on it being ready.
    #[must_use]
    pub(crate) const fn is_remote(&self) -> bool {
        matches!(self, Self::Remote { .. })
    }
}

/// The site of the hub and the collector: the server machine, or the single host.
#[must_use]
pub(crate) fn hub_site(cluster: &ClusterInput) -> Site {
    cluster
        .machines
        .first()
        .map_or(Site::SingleHost, |server| Site::Server {
            machine: server.name.clone(),
        })
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
    if machine.name == server.name {
        return Site::Server {
            machine: machine.name.clone(),
        };
    }
    Site::Remote {
        machine: machine.name.clone(),
        server_address: server.address.clone(),
    }
}

/// Reject a `machines:` list that does not describe one placement for every agent.
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
