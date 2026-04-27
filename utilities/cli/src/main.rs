use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use et_cli::{OutputType, generate_deployment, regenerate_verification};

#[derive(Parser)]
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
    },
    /// Regenerate verification outputs using verification input/output naming conventions.
    RegenVerification {
        #[arg(long, default_value = "verification")]
        verification_root: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::GenerateDeployment {
            input_file,
            output_dir,
            output_type,
        } => {
            println!("Reading cluster input from: {:?}", input_file);
            let summary = generate_deployment(input_file, output_dir, Some(*output_type))?;
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
        Commands::RegenVerification { verification_root } => {
            println!("Reading verification scenarios from: {:?}", verification_root);
            let regenerated = regenerate_verification(verification_root, None)?;
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
    }

    Ok(())
}
