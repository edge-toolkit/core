//! Generates what the firmware bakes in, read here from the build's environment, and links it for its board.
//!
//! Every build gets `modules.rs`, since the host serves the same modules a board would, and `guests.rs`, the runners it
//! starts beside the hub; a board's build also gets what [`board`] writes for it, and links as that says.

use std::env;
use std::error::Error;
use std::path::{Path, PathBuf};

use et_firmware_modules::{BOARD_VARIABLES, RUNNER_GUESTS, board, generate, guests};
use fs_err as fs;

/// The bytes a board with PSRAM holds for its guests' memories: its 8 MiB, less what wasmtime keeps beside them.
///
/// That is each guest's compiled code, copied in when it loads, its 64 KiB stack, and the store and instance, all of
/// which share the one heap with the hub's 160 KiB.
const PSRAM_GUEST_MEMORY: u64 = 6 * 1024 * 1024;
/// The flash `memory.x` gives the Pico 2 W, which the Pico Plus 2 W's build replaces with its own.
const PICO2W_FLASH: &str = "FLASH : ORIGIN = 0x10000000, LENGTH = 4096K";
/// The Pico Plus 2 W's 16 MiB of flash.
const PICO_PLUS2W_FLASH: &str = "FLASH : ORIGIN = 0x10000000, LENGTH = 16384K";
/// Set by cargo when the firmware is built for the Pimoroni Pico Plus 2 W, the board with PSRAM.
const CARGO_FEATURE_PICO_PLUS2W: &str = "CARGO_FEATURE_PICO_PLUS2W";
/// The pointer width cargo is building for, which picks the Pulley target the guests are compiled to.
const CARGO_CFG_TARGET_POINTER_WIDTH: &str = "CARGO_CFG_TARGET_POINTER_WIDTH";

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
/// The `address:port` of a hub on another machine, which makes the firmware a runner of its guests and no hub.
const HUB_ADDRESS: &str = "HUB_ADDRESS";
/// The cfg a build for a hub elsewhere is compiled with.
const REMOTE_HUB: &str = "remote_hub";
/// The operating system cargo is building for, `none` for a board.
const CARGO_CFG_TARGET_OS: &str = "CARGO_CFG_TARGET_OS";
/// The architecture cargo is building for, which picks the board.
const CARGO_CFG_TARGET_ARCH: &str = "CARGO_CFG_TARGET_ARCH";
/// The directory cargo gives this build script for what it generates.
const OUT_DIR: &str = "OUT_DIR";

/// Write `modules.rs`, and for a board what [`board`] writes, rerunning whenever a variable or file they read changes.
fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=memory.x");
    for variable in BOARD_VARIABLES
        .into_iter()
        .chain(MODULE_VARIABLES)
        .chain([RUNNER_GUESTS, HUB_ADDRESS])
    {
        println!("cargo:rerun-if-env-changed={variable}");
    }
    let out_dir = PathBuf::from(env::var(OUT_DIR)?);
    write_hub_address(&out_dir)?;
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
    let on_board = env::var(CARGO_CFG_TARGET_OS)? == "none";
    let with_psram = env::var(CARGO_FEATURE_PICO_PLUS2W).is_ok();
    // The host stands in for the board with PSRAM, the one that runs guests; any other board holds none.
    let guest_memory = if with_psram || !on_board { PSRAM_GUEST_MEMORY } else { 0 };
    let target = if env::var(CARGO_CFG_TARGET_POINTER_WIDTH)? == "32" {
        "pulley32"
    } else {
        "pulley64"
    };
    let runners = guests(
        &optional(MODULES_PATHS),
        &optional(RUNNER_GUESTS),
        target,
        guest_memory,
        &out_dir,
    )?;
    for file in &runners.files {
        println!("cargo:rerun-if-changed={}", file.display());
    }
    fs::write(out_dir.join("guests.rs"), runners.source)?;
    if on_board {
        let arch = env::var(CARGO_CFG_TARGET_ARCH)?;
        let memory_x = include_str!("memory.x");
        // The Pico Plus 2 W is the same RP2350 layout with four times the flash.
        let memory_x = if with_psram {
            memory_x.replace(PICO2W_FLASH, PICO_PLUS2W_FLASH)
        } else {
            memory_x.to_owned()
        };
        for directive in board(&arch, &out_dir, memory_x.as_bytes(), |name| env::var(name))? {
            println!("{directive}");
        }
    }
    Ok(())
}

/// Write `hub_address.rs`, naming [`HUB_ADDRESS`], and compile the firmware as a runner for that hub when it is set.
///
/// With a hub elsewhere the firmware serves nothing, and its guests reach that hub instead of one beside them.
#[expect(
    clippy::single_call_fn,
    reason = "the hub step of main, named apart from the modules"
)]
fn write_hub_address(out_dir: &Path) -> Result<(), Box<dyn Error>> {
    println!("cargo::rustc-check-cfg=cfg({REMOTE_HUB})");
    let hub_address = optional(HUB_ADDRESS);
    if !hub_address.is_empty() {
        println!("cargo::rustc-cfg={REMOTE_HUB}");
    }
    let source = format!(
        "/// The hub this firmware's guests reach, `address:port`.\nconst HUB_ADDRESS: &str = {hub_address:?};\n"
    );
    fs::write(out_dir.join("hub_address.rs"), source)?;
    Ok(())
}

/// The variable `name`, empty when it is unset, since every module variable may be.
fn optional(name: &str) -> String {
    env::var(name).unwrap_or_default()
}
