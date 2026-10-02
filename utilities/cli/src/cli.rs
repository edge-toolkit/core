use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::OutputType;

#[derive(Parser)]
#[non_exhaustive]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the binary matches every subcommand, so adding one has to fail that match rather than fall through"
)]
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
    /// Print the directory holding a mise-staged npm package.
    NpmModulePath {
        /// Published package name, as it appears in the mise tool id (e.g. `onnxruntime-web`).
        #[arg(long)]
        package: String,
    },
}
