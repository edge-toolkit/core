#![expect(clippy::print_stdout, reason = "CLI tool: println! is the intended UX")]

use std::path::Path;

use clap::{CommandFactory as _, Parser as _};
use et_cli::cli::{Cli, Commands};
use et_cli::{
    CliError, OutputType, block_ip, evict_agent, generate_deployment, generate_module_package_json, npm_module_path,
    regenerate_verification,
};

fn main() -> Result<(), CliError> {
    let cli = Cli::parse();

    let Some(command) = cli.command.as_ref() else {
        Cli::command().print_help()?;
        return Ok(());
    };

    match command {
        Commands::GenerateDeployment {
            input_file,
            output_dir,
            output_type,
        } => generate(input_file, output_dir, *output_type)?,
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
        Commands::EvictAgent {
            hub_url,
            agent_id,
            admin_token,
        } => println!("{}", evict_agent(hub_url, agent_id, admin_token)?),
        Commands::BlockIp {
            hub_url,
            ip,
            admin_token,
        } => {
            block_ip(hub_url, *ip, admin_token)?;
            println!("Blocked IP {ip}");
        }
        Commands::NpmModulePath { package } => {
            // Bare path on stdout, nothing else: generated deployments capture this in a `$(...)`.
            println!("{}", npm_module_path(package)?.display());
        }
    }

    Ok(())
}

/// Run `et-cli generate-deployment`: render `input_file` into `output_dir` as `output_type`, reporting what it wrote.
#[expect(
    clippy::single_call_fn,
    reason = "the generate-deployment command, kept out of main's dispatch so main stays a list of commands"
)]
fn generate(input_file: &Path, output_dir: &Path, output_type: OutputType) -> Result<(), CliError> {
    println!("Reading cluster input from: {}", input_file.display());
    let summary = generate_deployment(input_file, output_dir, Some(output_type))?;
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
    Ok(())
}
