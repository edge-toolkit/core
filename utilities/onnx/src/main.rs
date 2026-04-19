use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the ONNX model file.
    #[arg(short, long)]
    filename: PathBuf,
}

fn main() {
    //} -> std::io::Result<()> {
    let args = Args::parse();

    let model = onnx_extractor::OnnxModel::load_from_file(&args.filename.to_string_lossy()).unwrap();

    model.print_summary();
    model.print_model_info();
}
