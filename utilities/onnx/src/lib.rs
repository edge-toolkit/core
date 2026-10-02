//! The `et-onnx` command line, split into a lib target so et-int-gen can read its clap tree to write HELP.md.

use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
#[non_exhaustive]
pub struct Args {
    /// Path to the ONNX model file.
    #[arg(short, long)]
    pub filename: Option<PathBuf>,
}
