use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the ONNX model file.
    #[arg(short, long)]
    filename: Option<PathBuf>,

    /// Print the full clap help tree as markdown (used to regenerate HELP.md).
    #[cfg(feature = "markdown-help")]
    #[arg(long, hide = true)]
    markdown_help: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    #[cfg(feature = "markdown-help")]
    if args.markdown_help {
        clap_markdown::print_help_markdown::<Args>();
        return Ok(());
    }

    let Some(filename) = args.filename.as_ref() else {
        return Err("--filename is required".into());
    };

    let model = onnx_extractor::Model::load_from_file(filename)?;

    println!("{model}");
    Ok(())
}
