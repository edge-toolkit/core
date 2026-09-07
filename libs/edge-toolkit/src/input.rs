use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ClusterInput {
    pub cluster_name: String,
    #[serde(default)]
    pub deployment_type: Option<String>,
    pub agents: Vec<Agent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Agent {
    pub name: String,
    /// Runner that executes this agent's modules, from `runner:`.
    ///
    /// Unset means the modules are only served, for a browser to load and run itself -- which is what every
    /// deployment did before this field existed. Set, the generated deployment also starts a runner process per
    /// resource, so the cluster runs headless.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<String>,
    pub resources: Vec<Resource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Resource {
    #[serde(rename = "type")]
    pub resource_type: String,
}
