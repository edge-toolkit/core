//! Put the probe sets to every GPU adapter of this machine and to its CPU, and add to the registry each sighting it
//! does not yet hold.
//!
//! It takes no arguments. The registry is the repository's own `libs/gpu-probe/signatures.yaml`, found from the working
//! directory. The adapters are every one wgpu finds through Vulkan, Metal and Direct3D 12, software renderers too.

use et_gpu_probe::native::Gpu;
use et_gpu_probe_server::measure::measure;
use et_gpu_probe_server::registry::Registry;
use futures_executor::block_on;

/// The registry file, from the repository's root.
const REGISTRY: &str = "libs/gpu-probe/signatures.yaml";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let registry = Registry::new(edge_toolkit::config::get_project_root().join(REGISTRY));
    measure(&registry, block_on(Gpu::open_all()), &mut std::io::stdout().lock())?;
    Ok(())
}
