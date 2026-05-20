// See lib.rs — Error inherits ureq::Error's bulk; immaterial for a CLI.
#![allow(clippy::result_large_err)]

//! CLI entrypoint for `et-int-gen`. All real work lives in the library
//! (`et_int_gen`); this file just parses arguments and dispatches.

use clap::{Parser, Subcommand};
use edge_toolkit::config::get_project_root;
use et_int_gen::{generate, wit::upstream};

#[derive(Parser)]
#[command(about = "Generate checked-in artifacts under generated/ from in-repo Rust sources of truth")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Emit the AsyncAPI/OpenAPI YAML, WIT, KDL, and Rust REST client (default).
    Generate,
    /// Fetch upstream WASI WIT packages into generated/specs/wit/ at pinned versions.
    FetchDeps,
}

fn main() -> Result<(), et_int_gen::Error> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Generate) {
        Command::Generate => generate(),
        Command::FetchDeps => upstream::run(&get_project_root()),
    }
}
