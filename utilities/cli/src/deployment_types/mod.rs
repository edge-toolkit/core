mod dekit;
mod docker_compose;
mod hub;
mod k3s;
mod k3s_recovery;
mod mise;
mod process_compose;
mod scenario_image;
mod yaml;

pub use self::dekit::generate_dekit_deployment;
pub(crate) use self::docker_compose::{HUB_IMAGE_BUILD, SCHEDULER_SERVICE, scheduler_service};
pub use self::docker_compose::{docker_image_module_paths, generate_docker_compose_deployment};
pub(crate) use self::hub::{hub_root_module, serves_a_page};
pub use self::k3s::generate_k3s_deployment;
pub(crate) use self::k3s::{
    CONTAINERD_TEMPLATE_FILE, PLUGIN_NAMESPACE as K3S_PLUGIN_NAMESPACE, needs_containerd_template,
    node_preparation_note as k3s_node_preparation_note,
};
pub(crate) use self::mise::{COLLECTOR_TASK, HUB_TASK, OPENER_TASK, STORAGE_DIR, machine_task};
pub use self::mise::{ScenarioModules, generate_mise_deployment, scenario_module_paths};
pub use self::process_compose::generate_process_compose_deployment;
pub use self::scenario_image::generate_scenario_image;
pub(crate) use self::scenario_image::{ImageNeeds, REGISTRY_TOKEN_ENV, REGISTRY_TOKEN_SECRET, scenario_image_needs};
use self::yaml::document;
