#![expect(
    clippy::print_stdout,
    reason = "CLI tool: the model description on stdout is its whole output"
)]

use clap::Parser as _;
use et_onnx::Args;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let Some(filename) = args.filename.as_ref() else {
        return Err("--filename is required".into());
    };

    let model = onnx_extractor::Model::load_from_file(filename)?;

    println!("{model}");
    Ok(())
}
