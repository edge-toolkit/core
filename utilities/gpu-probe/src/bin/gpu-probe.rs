//! Put the probe sets to every GPU adapter of this machine and to its CPU, and record each sighting.
//!
//! By default each one the registry does not yet hold is added to the repository's own `libs/gpu-probe/signatures`,
//! found from the working directory; with `--into` they are written as a file of submissions instead, for a registry
//! this cannot write to, such as on a CI runner. The adapters are every one wgpu finds through Vulkan, Metal and
//! Direct3D 12, software renderers too.

use std::path::PathBuf;

use clap::Parser as _;
use et_gpu_probe::native::Gpu;
use et_gpu_probe_server::machine::Machine;
use et_gpu_probe_server::measure::{measure, propose_measured};
use et_gpu_probe_server::registry::RegistryTree;
use futures_executor::block_on;

/// Put the probe sets to every GPU adapter of this machine and to its CPU, and record each sighting.
#[derive(Debug, clap::Parser)]
#[command(name = "gpu-probe", version)]
struct Args {
    /// The machine the rows name, such as its make and model.
    ///
    /// A GitHub Actions runner is named by its runner image and build instead, whatever this says.
    #[arg(long, default_value = "")]
    device: String,
    /// A directory to write the signatures into as a file of submissions, rather than add them to the registry.
    ///
    /// Absent, the repository's registry is added to.
    #[arg(long)]
    into: Option<PathBuf>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let machine = Machine::here(&args.device)?;
    let opened = block_on(Gpu::open_all());
    let mut out = std::io::stdout().lock();
    match args.into {
        Some(dir) => {
            let _path: PathBuf = propose_measured(&dir, &machine, opened, &mut out)?;
        }
        None => measure(&RegistryTree::repository(), &machine, opened, &mut out)?,
    }
    Ok(())
}
