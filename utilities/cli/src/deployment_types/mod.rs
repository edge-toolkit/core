use std::path::Path;

mod docker_compose;
mod k3s;
mod mise;
mod scenario_image;

use crate::input::ClusterInput;

/// Whether this cluster's hub is opened in a browser, and so needs the page module and what it imports.
///
/// An agent with no `runner` is one whose modules a browser loads, and a cluster with no agents at all is the bare hub,
/// which exists to serve that page. Everything else runs headless: the hub relays between runner processes and nothing
/// ever opens it, so a front page there would provision a module nobody loads, along with the runtimes its first import
/// pulls in.
pub(crate) fn serves_a_page(cluster: &ClusterInput) -> bool {
    cluster.agents.is_empty()
        || cluster
            .agents
            .iter()
            .any(|agent| agent.runner.as_deref().is_none_or(|kind| kind.trim().is_empty()))
}

/// The module the hub serves at `/`, named as its own `package.json` declares it.
///
/// Read from that manifest rather than written out, so a generated deployment carries whatever the page module is
/// actually called -- including the owner scope publishing puts on it -- without this crate holding a second copy
/// of the name to drift from it. Every deployment format that serves the page needs it: the hub has no default for
/// which module is a deployment's front page, so the deployment that knows names it.
pub(crate) fn hub_root_module(ws_server_dir: &Path) -> String {
    crate::module_package_json(&ws_server_dir.join("static"))
        .and_then(|package| package.name)
        .unwrap_or_default()
}

pub use self::docker_compose::{docker_image_module_paths, generate_docker_compose_deployment};
pub use self::k3s::generate_k3s_deployment;
pub use self::mise::{ScenarioModules, generate_mise_deployment, scenario_module_paths};
pub use self::scenario_image::generate_scenario_image;
