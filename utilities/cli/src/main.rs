#![expect(clippy::print_stdout, reason = "CLI tool: println! is the intended UX")]

use clap::Parser as _;
use et_cli::{CliError, generate_deployment, generate_module_package_json, regenerate_verification};

mod cli;

use crate::cli::{Cli, Commands};

fn main() -> Result<(), CliError> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::GenerateDeployment {
            input_file,
            output_dir,
            output_type,
        } => {
            println!("Reading cluster input from: {}", input_file.display());
            let summary = generate_deployment(input_file, output_dir, Some(*output_type))?;
            println!(
                "Scenario summary: input={}, cluster={}, agents={}, resources={}",
                input_file.display(),
                summary.cluster_name,
                summary.agent_templates,
                summary.module_names.join(", ")
            );
            println!(
                "Generated: {}",
                output_dir.join(output_type.output_file_name()).display()
            );
            println!(
                "See the generated README.md in {} for instructions.",
                output_dir.display()
            );
        }
        Commands::RegenVerification { verification_root } => {
            println!("Reading verification scenarios from: {}", verification_root.display());
            let regenerated = regenerate_verification(verification_root, None)?;
            for scenario in &regenerated {
                println!(
                    "Regenerated: input={}, output={}, cluster={}, agents={}, resources={}",
                    scenario.input_file.display(),
                    scenario.output_dir.display(),
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
