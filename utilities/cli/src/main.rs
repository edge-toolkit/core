use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use et_cli::{
    DeploymentMode, DeploymentOptions, OutputType, generate_deployment_with_options, generate_module_package_json,
    regenerate_verification_with_options,
};

#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate deployment config from a cluster input YAML.
    GenerateDeployment {
        #[arg(long)]
        input_file: PathBuf,
        #[arg(long)]
        output_dir: PathBuf,
        #[arg(long, value_enum, default_value_t)]
        output_type: OutputType,
        #[arg(long, value_enum, default_value_t)]
        mode: DeploymentMode,
        #[arg(long, help = "Path to the installed edge-toolkit runtime bundle in published mode")]
        edge_toolkit_path: Option<PathBuf>,
    },
    /// Regenerate verification outputs using verification input/output naming conventions.
    RegenVerification {
        #[arg(long, default_value = "verification")]
        verification_root: PathBuf,
        #[arg(long, value_enum, default_value_t)]
        mode: DeploymentMode,
        #[arg(long, help = "Path to the installed edge-toolkit runtime bundle in published mode")]
        edge_toolkit_path: Option<PathBuf>,
    },
    /// Generate pkg/package.json from module metadata.
    ModulePackageJson {
        #[arg(long, default_value = ".")]
        module_dir: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::GenerateDeployment {
            input_file,
            output_dir,
            output_type,
            mode,
            edge_toolkit_path,
        } => {
            println!("Reading cluster input from: {:?}", input_file);
            let options = DeploymentOptions {
                mode: *mode,
                edge_toolkit_path: edge_toolkit_path.clone(),
            };
            let summary = generate_deployment_with_options(input_file, output_dir, Some(*output_type), &options)?;
            println!(
                "Scenario summary: input={:?}, cluster={}, agents={}, resources={}",
                input_file,
                summary.cluster_name,
                summary.agent_templates,
                summary.module_names.join(", ")
            );
            println!("Generated: {:?}", output_dir.join(output_type.output_file_name()));
            println!("See the generated README.md in {:?} for instructions.", output_dir);
        }
        Commands::RegenVerification {
            verification_root,
            mode,
            edge_toolkit_path,
        } => {
            println!("Reading verification scenarios from: {:?}", verification_root);
            let options = DeploymentOptions {
                mode: *mode,
                edge_toolkit_path: edge_toolkit_path.clone(),
            };
            let regenerated = regenerate_verification_with_options(verification_root, None, &options)?;
            for scenario in &regenerated {
                println!(
                    "Regenerated: input={:?}, output={:?}, cluster={}, agents={}, resources={}",
                    scenario.input_file,
                    scenario.output_dir,
                    scenario.summary.cluster_name,
                    scenario.summary.agent_templates,
                    scenario.summary.module_names.join(", ")
                );
            }
            println!("Regenerated {} verification scenario output set(s).", regenerated.len());
        }
        Commands::ModulePackageJson { module_dir } => {
            let output_path = generate_module_package_json(module_dir)?;
            println!("Wrote {}", output_path.display());
        }
    }

    Ok(())
}
