//! File every submission waiting in the registry's `submitted/` directory under `curated/`, saying what became of each.
//!
//! It takes no arguments. The registry is the repository's own `libs/gpu-probe/signatures`, found from the working
//! directory. Each submitted file is removed once all of it is filed.

use std::io::Write as _;

use et_gpu_probe_server::registry::RegistryTree;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut out = std::io::stdout().lock();
    for (path, outcomes) in RegistryTree::repository().fold()? {
        for outcome in outcomes {
            writeln!(out, "{}: {outcome}", path.display())?;
        }
    }
    Ok(())
}
