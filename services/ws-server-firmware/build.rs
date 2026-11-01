//! Generates what the firmware bakes in, read here from the build's environment, and links it for its board.
//!
//! Every build gets `modules.rs`, since the host serves the same modules a board would; a board's build also gets
//! what [`board`] writes for it, and links as that says.

use std::env;
use std::error::Error;
use std::path::PathBuf;

use et_firmware_modules::{BOARD_VARIABLES, board, generate};
use fs_err as fs;

/// The variables `modules.rs` is generated from, each named as the `std` server's modules service or `MODULES_*` reads.
const MODULE_VARIABLES: [&str; 4] = [MODULES_PATHS, MODULES_ROOT, MODULES_ELSEWHERE, MODULES_ELSEWHERE_URL];
/// The module directories to build in, comma-separated, as the `std` server's `MODULES_PATHS` lists them.
const MODULES_PATHS: &str = "MODULES_PATHS";
/// The name of the module served at `/`, as the `std` server's `MODULES_ROOT`.
const MODULES_ROOT: &str = "MODULES_ROOT";
/// The names of the modules served from `MODULES_ELSEWHERE_URL` rather than built in, comma-separated.
const MODULES_ELSEWHERE: &str = "MODULES_ELSEWHERE";
/// Where a request for a module in `MODULES_ELSEWHERE` is redirected, its path under `/modules` appended.
const MODULES_ELSEWHERE_URL: &str = "MODULES_ELSEWHERE_URL";
/// The operating system cargo is building for, `none` for a board.
const CARGO_CFG_TARGET_OS: &str = "CARGO_CFG_TARGET_OS";
/// The architecture cargo is building for, which picks the board.
const CARGO_CFG_TARGET_ARCH: &str = "CARGO_CFG_TARGET_ARCH";
/// The directory cargo gives this build script for what it generates.
const OUT_DIR: &str = "OUT_DIR";

/// Write `modules.rs`, and for a board what [`board`] writes, rerunning whenever a variable or file they read changes.
fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=memory.x");
    for variable in BOARD_VARIABLES.into_iter().chain(MODULE_VARIABLES) {
        println!("cargo:rerun-if-env-changed={variable}");
    }
    let out_dir = PathBuf::from(env::var(OUT_DIR)?);
    // The modules are served on the host too, so what a board would serve can be explored without one.
    let modules = generate(
        &optional(MODULES_PATHS),
        &optional(MODULES_ROOT),
        &optional(MODULES_ELSEWHERE),
        &optional(MODULES_ELSEWHERE_URL),
    )?;
    for file in &modules.files {
        println!("cargo:rerun-if-changed={}", file.display());
    }
    fs::write(out_dir.join("modules.rs"), modules.source)?;
    if env::var(CARGO_CFG_TARGET_OS)? == "none" {
        let arch = env::var(CARGO_CFG_TARGET_ARCH)?;
        for directive in board(&arch, &out_dir, include_bytes!("memory.x"), |name| env::var(name))? {
            println!("{directive}");
        }
    }
    Ok(())
}

/// The variable `name`, empty when it is unset, since every module variable may be.
fn optional(name: &str) -> String {
    env::var(name).unwrap_or_default()
}
