//! The `et-int-gen` command line, kept in the library so [`crate::help`] can read its clap tree to write HELP.md.

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(about = "Generate checked-in artifacts (generated/, CHECKS.md, HELP.md) from in-repo sources of truth")]
#[non_exhaustive]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the binary matches every subcommand, so adding one has to fail that match rather than fall through"
)]
pub enum Command {
    /// Emit the generated artifacts for one target (default: all).
    Generate {
        /// Which artifacts to emit; defaults to `all`.
        #[arg(value_enum, default_value_t = Target::All)]
        target: Target,
    },
    /// Fetch upstream WASI WIT packages into generated/specs/wit/ at pinned versions.
    FetchDeps,
    /// Write CHECKS.md, the catalogue of every check across every `MISE_ENV`.
    Checks {
        /// Compare against the committed CHECKS.md and fail on drift instead of writing it.
        #[arg(long)]
        check: bool,
    },
    /// Write each binary crate's HELP.md from its clap command tree and its environment config.
    HelpMd {
        /// Compare against the committed HELP.md files and fail on drift instead of writing them.
        #[arg(long)]
        check: bool,
    },
}

/// Per-language target selector for the `generate` subcommand.
///
/// Mirrors the `MISE_ENV`-scoped `gen:*` tasks: `core` (language-agnostic specs), `rust`, `bindings`, `zig`, or `all`.
#[derive(Clone, Copy, ValueEnum)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the binary matches every target, so adding one has to fail that match rather than fall through"
)]
pub enum Target {
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
