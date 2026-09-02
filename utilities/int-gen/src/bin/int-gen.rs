//! CLI entrypoint for `et-int-gen`. All real work lives in the library
//! (`et_int_gen`); this file just parses arguments and dispatches.

use clap::{Parser, Subcommand, ValueEnum};
use edge_toolkit::config::get_project_root;
use et_int_gen::{generate, generate_bindings, generate_core, generate_rust, generate_zig, wit::upstream};

#[derive(Parser)]
#[command(about = "Generate checked-in artifacts under generated/ from in-repo Rust sources of truth")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Print the full clap help tree as markdown (used to regenerate HELP.md).
    #[cfg(feature = "markdown-help")]
    #[arg(long, hide = true)]
    markdown_help: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Emit the generated artifacts for one target (default: all).
    Generate {
        /// Which artifacts to emit; defaults to `all`.
        #[arg(value_enum, default_value_t = Target::All)]
        target: Target,
    },
    /// Fetch upstream WASI WIT packages into generated/specs/wit/ at pinned versions.
    FetchDeps,
}

/// Per-language target selector for the `generate` subcommand, mirroring the
/// `MISE_ENV`-scoped `gen:*` tasks: `core` (language-agnostic specs), `rust`,
/// `bindings`, `zig`, or `all`.
#[derive(Clone, Copy, ValueEnum)]
enum Target {
    /// Language-agnostic specs: AsyncAPI/OpenAPI YAML, WIT, KDL, schema JSON.
    Core,
    /// The typed Rust REST client.
    Rust,
    /// The wasmtime host bindings for the ws-wasi-runner `runner` world.
    Bindings,
    /// The Zig REST client (skipped when openapi2zig is absent).
    Zig,
    /// Core + Rust + bindings + Zig.
    All,
}

fn main() -> Result<(), et_int_gen::Error> {
    let cli = Cli::parse();

    #[cfg(feature = "markdown-help")]
    if cli.markdown_help {
        clap_markdown::print_help_markdown::<Cli>();
        return Ok(());
    }

    match cli.command.unwrap_or(Command::Generate { target: Target::All }) {
        Command::Generate { target } => match target {
            Target::Core => generate_core(),
            Target::Rust => generate_rust(),
            Target::Bindings => generate_bindings(),
            Target::Zig => generate_zig(),
            Target::All => generate(),
        },
        Command::FetchDeps => upstream::run(&get_project_root()),
    }
}
