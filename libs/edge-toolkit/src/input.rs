use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterInput {
    pub cluster_name: String,
    #[serde(default)]
    pub deployment_type: Option<String>,
    pub agents: Vec<AgentTemplate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTemplate {
    pub name: String,
    #[serde(default = "default_count")]
    pub count: u32,
    pub capabilities: Vec<String>,
    pub modules: Vec<ModuleInput>,
}

fn default_count() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleInput {
    pub name: String,
    pub config: Option<HashMap<String, serde_yaml::Value>>,
}
