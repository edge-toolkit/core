mod dekit;
mod docker_compose;
mod hub;
mod k3s;
mod mise;
mod scenario_image;
mod yaml;

pub use self::dekit::generate_dekit_deployment;
pub use self::docker_compose::{docker_image_module_paths, generate_docker_compose_deployment};
pub(crate) use self::hub::{hub_root_module, serves_a_page};
pub use self::k3s::generate_k3s_deployment;
pub(crate) use self::mise::{COLLECTOR_TASK, HUB_TASK, OPENER_TASK, STORAGE_DIR};
pub use self::mise::{ScenarioModules, generate_mise_deployment, scenario_module_paths};
pub use self::scenario_image::generate_scenario_image;
use self::yaml::document;
