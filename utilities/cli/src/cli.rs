use std::path::PathBuf;

use clap::{Parser, Subcommand};
use et_cli::OutputType;

#[derive(Parser)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Generate deployment config from a cluster input YAML.
    GenerateDeployment {
        #[arg(long)]
        input_file: PathBuf,
        #[arg(long)]
        output_dir: PathBuf,
        #[arg(long, value_enum, default_value_t)]
        output_type: OutputType,
    },
    /// Regenerate verification outputs using verification input/output naming conventions.
    RegenVerification {
        #[arg(long, default_value = "verification")]
        verification_root: PathBuf,
    },
    /// Generate pkg/package.json from module metadata.
    ModulePackageJson {
        #[arg(long, default_value = ".")]
        module_dir: PathBuf,
    },
}
