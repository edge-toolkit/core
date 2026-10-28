//! The ports a scenario publishes outside its cluster, the checks that keep two of them apart, and their addresses.
//!
//! Only a k3s deployment has a boundary to publish across: every other format runs its processes on the host's own
//! network, where a port a process opens is already open. The checks still run for every format, because two
//! processes on one machine binding the same port collide there too -- on the host network they collide whether or
//! not anything declared the port, which is why the hub's and the collector's ports are always reserved.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use edge_toolkit::ports::Services as Ports;

use crate::error::CliError;
use crate::input::{Agent, ClusterInput, ExposedPort, Protocol};
use crate::placement::{Site, agent_site, hub_site, hubs};
use crate::{deployment_types, rfc1123_label};

/// The hub's ports a scenario may expose, by the name `services.hub.expose` lists them with.
const HUB_PORTS: [(&str, Ports); 2] = [
    ("ws", Ports::InsecureWebSocketServer),
    ("wss", Ports::SecureWebSocketServer),
];

/// Longest port name, which is what Kubernetes allows a container or service port's name.
const PORT_NAME_MAX: usize = 15;

/// Longest Kubernetes `Service` name, the RFC 1035 label every `Service` is named with.
const SERVICE_NAME_MAX: usize = 63;

/// The runner kinds that cannot accept a connection, each with why not.
///
/// The wasi runner links `wasi:sockets` but builds its context without network access, so every bind is refused.
const NON_LISTENING_RUNNERS: [(&str, &str); 1] = [(
    "wasi",
    "the wasi runner grants its component no network access to listen with",
)];

/// The hub's exposed ports, in the order [`HUB_PORTS`] lists them.
///
/// A name `services.hub.expose` lists that is not the hub's has already been rejected by [`validate`].
pub(crate) fn hub_ports(cluster: &ClusterInput) -> Vec<ExposedPort> {
    HUB_PORTS
        .iter()
        .filter(|(name, _)| cluster.services.hub.expose.iter().any(|listed| listed == name))
        .map(|(name, port)| ExposedPort {
            name: (*name).to_owned(),
            port: port.port(),
            protocol: Protocol::Tcp,
        })
        .collect()
}

/// The k3s `Service` that publishes a hub's exposed ports, beside the in-cluster one its runners resolve.
///
/// A second `Service` rather than the hub's own turned into a load balancer, so the ports the scenario did not expose
/// stay inside the cluster while the runners still reach every one of them.
pub(crate) fn external_hub_service(hub: &str) -> String {
    format!("{hub}-ext")
}

/// Whether the scenario publishes anything, which is what has the k3s machines join a load-balancer pool each.
pub(crate) fn any_exposed(cluster: &ClusterInput) -> bool {
    !cluster.services.hub.expose.is_empty() || cluster.agents.iter().any(|agent| !agent.expose.is_empty())
}

/// Reject an exposure no format can honour, or one that collides with another process's port on its machine.
pub(crate) fn validate(cluster: &ClusterInput) -> Result<(), CliError> {
    validate_hub(cluster)?;
    for agent in cluster.agents.iter().filter(|agent| !agent.expose.is_empty()) {
        validate_listener(agent)?;
        validate_agent_ports(agent)?;
    }
    validate_collisions(cluster)
}

/// Reject an exposing agent whose process cannot hold a port open for the scenario's lifetime.
#[expect(
    clippy::single_call_fn,
    reason = "the listener step of validate, separate for readability"
)]
fn validate_listener(agent: &Agent) -> Result<(), CliError> {
    let runner = agent.runner.as_deref().map_or("", str::trim);
    let unsupported = |reason: &str| CliError::ExposeUnsupported {
        agent: agent.name.clone(),
        reason: reason.to_owned(),
    };
    if runner.is_empty() {
        return Err(unsupported("names no runner, and a browser cannot accept a connection"));
    }
    if let Some((_, reason)) = NON_LISTENING_RUNNERS.iter().find(|(kind, _)| *kind == runner) {
        return Err(unsupported(reason));
    }
    if agent.resources.len() > 1 {
        return Err(unsupported(
            "has more than one resource, whose runners would share one environment and bind the same ports",
        ));
    }
    if !agent.schedule.is_unset() {
        return Err(unsupported(
            "sets a schedule, and a runner started per tick is not a server",
        ));
    }
    Ok(())
}

/// Reject an agent port that is badly named, numbered 0, or named twice.
#[expect(
    clippy::single_call_fn,
    reason = "the port step of validate, separate for readability"
)]
fn validate_agent_ports(agent: &Agent) -> Result<(), CliError> {
    for (index, exposed) in agent.expose.iter().enumerate() {
        let bad_name = |reason: String| CliError::InvalidExposedPortName {
            agent: agent.name.clone(),
            name: exposed.name.clone(),
            reason,
        };
        rfc1123_label(&exposed.name, PORT_NAME_MAX, bad_name)?;
        // A port name is narrower than a label: Kubernetes rejects one that could be read as a number, or that has
        // consecutive hyphens.
        if !exposed.name.chars().any(|found| found.is_ascii_lowercase()) {
            return Err(bad_name("it has no letter".to_owned()));
        }
        if exposed.name.contains("--") {
            return Err(bad_name("it contains consecutive '-'".to_owned()));
        }
        if exposed.port == 0 {
            return Err(CliError::ZeroExposedPort {
                agent: agent.name.clone(),
                name: exposed.name.clone(),
            });
        }
        if agent
            .expose
            .iter()
            .take(index)
            .any(|earlier| earlier.name == exposed.name)
        {
            return Err(CliError::DuplicateExposedPortName {
                owner: format!("Agent {:?}", agent.name),
                name: exposed.name.clone(),
            });
        }
    }
    Ok(())
}

/// Reject a `services.hub.expose` naming a port the hub does not have, or one twice.
#[expect(
    clippy::single_call_fn,
    reason = "the hub step of validate, separate for readability"
)]
fn validate_hub(cluster: &ClusterInput) -> Result<(), CliError> {
    let listed = &cluster.services.hub.expose;
    for (index, name) in listed.iter().enumerate() {
        if !HUB_PORTS.iter().any(|(known, _)| known == name) {
            let known = HUB_PORTS.map(|(known, _)| known).join(", ");
            return Err(CliError::UnknownHubPort {
                name: name.clone(),
                known,
            });
        }
        if listed.iter().take(index).any(|earlier| earlier == name) {
            return Err(CliError::DuplicateExposedPortName {
                owner: "services.hub".to_owned(),
                name: name.clone(),
            });
        }
    }
    if listed.is_empty() {
        return Ok(());
    }
    for hub in hubs(cluster) {
        let service = external_hub_service(&hub.name);
        rfc1123_label(&service, SERVICE_NAME_MAX, |reason| CliError::InvalidHubName {
            machine: hub.site.machine_names().join(""),
            hub: service.clone(),
            reason,
        })?;
    }
    Ok(())
}

/// Reject two processes on one machine using the same port.
///
/// The hub's and the collector's ports are held whether or not the scenario exposes them, since on the host network
/// they are bound either way.
#[expect(
    clippy::single_call_fn,
    reason = "the collision step of validate, separate for readability"
)]
fn validate_collisions(cluster: &ClusterInput) -> Result<(), CliError> {
    let mut taken: BTreeMap<(String, u16, Protocol), String> = BTreeMap::new();
    let mut claim = |site: &Site, port: u16, protocol: Protocol, owner: String| {
        let machine = site.machine_names().join("");
        let key = (machine.clone(), port, protocol);
        if let Some(first) = taken.get(&key) {
            return Err(CliError::ExposedPortCollision {
                port,
                protocol: protocol.name().to_owned(),
                machine: if machine.is_empty() {
                    "this host".to_owned()
                } else {
                    format!("machine {machine:?}")
                },
                first: first.clone(),
                second: owner,
            });
        }
        let _previous = taken.insert(key, owner);
        Ok(())
    };
    claim(
        &hub_site(cluster),
        Ports::OtlpCollector.port(),
        Protocol::Tcp,
        format!("`{}`", deployment_types::COLLECTOR_TASK),
    )?;
    for hub in hubs(cluster) {
        for (_, port) in HUB_PORTS {
            claim(&hub.site, port.port(), Protocol::Tcp, format!("`{}`", hub.name))?;
        }
    }
    for agent in &cluster.agents {
        let site = agent_site(cluster, &agent.name);
        for exposed in &agent.expose {
            claim(&site, exposed.port, exposed.protocol, format!("Agent {:?}", agent.name))?;
        }
    }
    Ok(())
}

/// Render the list of every exposed port, as the address it is reached at from outside, or nothing when none is.
///
/// A port named after a web scheme is a link with that scheme; anything else is its address and transport. The hub's
/// are left out unless `with_hubs`, for a README that already gives the hub's addresses itself.
pub(crate) fn address_list(cluster: &ClusterInput, with_hubs: bool) -> String {
    let mut list = String::default();
    let hub_ports = if with_hubs { hub_ports(cluster) } else { Vec::new() };
    for hub in hubs(cluster) {
        for exposed in &hub_ports {
            push_address(&mut list, cluster, &hub.name, &hub.site, exposed);
        }
    }
    for agent in &cluster.agents {
        let site = agent_site(cluster, &agent.name);
        for exposed in &agent.expose {
            push_address(&mut list, cluster, &agent.name, &site, exposed);
        }
    }
    list
}

/// Append one exposed port of `owner` at `site` to the list [`address_list`] renders.
fn push_address(list: &mut String, cluster: &ClusterInput, owner: &str, site: &Site, exposed: &ExposedPort) {
    let machine = site.machine_names().join("");
    let host = cluster
        .machines
        .iter()
        .find(|candidate| candidate.name == machine)
        .map_or("localhost", |found| found.address.as_str());
    let ExposedPort { name, port, protocol } = exposed;
    let scheme = match (name.as_str(), protocol) {
        ("http" | "ws", Protocol::Tcp) => "http",
        ("https" | "wss", Protocol::Tcp) => "https",
        _ => "",
    };
    let address = if scheme.is_empty() {
        format!("`{host}:{port}/{}`", protocol.name())
    } else {
        format!("<{scheme}://{host}:{port}/>")
    };
    let _write_result = writeln!(list, "- `{owner}` `{name}`: {address}");
}
