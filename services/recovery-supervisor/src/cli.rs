//! Command-line configuration location; operational policy is explicit in the TOML document.

use std::path::PathBuf;

/// Supervise configured workloads through the hub and an explicitly scoped Kubernetes backend.
#[derive(clap::Parser)]
#[command(version, about)]
#[non_exhaustive]
pub struct Args {
    /// Recovery configuration file.
    pub config: PathBuf,
}
