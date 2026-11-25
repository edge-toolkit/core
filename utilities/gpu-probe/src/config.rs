//! What a measurement reads from the environment: the GitHub Actions runner it may run on, and where to propose what
//! it measured.

use serde::Deserialize;

/// Every variable a measurement reads, each empty where it is not set.
#[derive(Debug, Default, Deserialize)]
#[cfg_attr(feature = "env-schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Config {
    /// The runner image's name, such as `macos15` or `ubuntu24`, which a GitHub Actions runner sets as `ImageOS`.
    #[serde(default)]
    pub imageos: String,
    /// The runner image's build, such as `20261110.1`, which a GitHub Actions runner sets as `ImageVersion`.
    #[serde(default)]
    pub imageversion: String,
    /// The run a GitHub Actions runner is part of.
    #[serde(default)]
    pub github: GitHub,
    /// Where a measurement proposes what it measured.
    #[serde(default)]
    pub gpu: Gpu,
}

/// The `GITHUB_*` variables that locate a run.
#[derive(Debug, Default, Deserialize)]
#[cfg_attr(feature = "env-schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct GitHub {
    /// The server, such as `https://github.com`.
    #[serde(default)]
    pub server_url: String,
    /// The repository, `owner/name`.
    #[serde(default)]
    pub repository: String,
    /// The run's number.
    #[serde(default)]
    pub run_id: String,
}

/// The `GPU_*` variables.
#[derive(Debug, Default, Deserialize)]
#[cfg_attr(feature = "env-schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Gpu {
    /// A directory a measurement writes what it measured into as a file of submissions; none is written when empty.
    #[serde(default)]
    pub probe_into: String,
    /// The machine a measurement names as its device, such as `Dell Precision 5570`, where it is given none itself.
    #[serde(default)]
    pub probe_device: String,
}

/// The environment, read.
pub fn load() -> Result<Config, serde_env::Error> {
    serde_env::from_env()
}
