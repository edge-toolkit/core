//! CLI entrypoint for `et-int-gen`. All real work lives in the library (`et_int_gen`); this file just parses arguments
//! and dispatches.

use clap::Parser as _;
use edge_toolkit::config::get_project_root;
use et_int_gen::cli::{Cli, Command, Target};
use et_int_gen::{
    check_checks, check_help, generate, generate_bindings, generate_checks, generate_core, generate_help,
    generate_rust, generate_zig, wit::upstream,
};

fn main() -> Result<(), et_int_gen::Error> {
    let cli = Cli::parse();

    match cli.command.unwrap_or(Command::Generate { target: Target::All }) {
        Command::Generate { target } => match target {
            Target::Core => generate_core(),
            Target::Rust => generate_rust(),
            Target::Bindings => generate_bindings(),
            Target::Zig => generate_zig(),
            Target::All => generate(),
        },
        Command::FetchDeps => upstream::run(&get_project_root()),
        Command::Checks { check: true } => check_checks(),
        Command::Checks { check: false } => generate_checks(),
        Command::HelpMd { check: true } => check_help(),
        Command::HelpMd { check: false } => generate_help(),
    }
}
