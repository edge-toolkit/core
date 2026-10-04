//! The hub's command-line arguments; everything else it reads from the environment into [`crate::config::Config`].

use std::path::PathBuf;

use clap::Parser;

/// Command-line arguments for `et-ws-server`.
#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
#[non_exhaustive]
pub struct Args {
    /// Path to agent registry YAML file.
    #[arg(short, long, default_value = "registry.yaml")]
    pub agent_registry: PathBuf,
}
