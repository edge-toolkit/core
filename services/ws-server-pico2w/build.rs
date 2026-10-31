//! Puts the Pico 2 W's memory layout where the linker looks, links the firmware with cortex-m-rt's and defmt's
//! linker scripts, and generates `board.rs` and `modules.rs`: what the firmware bakes in, read here from the build's
//! environment. On any other target the binary serves the same modules from the host, so only `modules.rs` is made.

use std::env;
use std::error::Error;
use std::path::PathBuf;

use et_firmware_modules::{generate, include_path};
use fs_err as fs;

/// The variables `board.rs` is generated from.
const BOARD_VARIABLES: [&str; 3] = ["CYW43_FIRMWARE_DIR", "WIFI_NETWORK", "WIFI_PASSWORD"];
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
/// The operating system cargo is building for, `none` for the board.
const CARGO_CFG_TARGET_OS: &str = "CARGO_CFG_TARGET_OS";
/// The directory cargo gives this build script for what it generates.
const OUT_DIR: &str = "OUT_DIR";

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=memory.x");
    for variable in BOARD_VARIABLES.into_iter().chain(MODULE_VARIABLES) {
        println!("cargo:rerun-if-env-changed={variable}");
    }
    let out_dir = PathBuf::from(env::var(OUT_DIR)?);
    // The modules are served on the host too, so what the board would serve can be explored without one.
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
    if env::var(CARGO_CFG_TARGET_OS)? != "none" {
        return Ok(());
    }
    fs::write(out_dir.join("memory.x"), include_bytes!("memory.x"))?;
    fs::write(out_dir.join("board.rs"), board()?)?;
    println!("cargo:rustc-link-search={}", out_dir.display());
    println!("cargo:rustc-link-arg-bins=--nmagic");
    println!("cargo:rustc-link-arg-bins=-Tlink.x");
    println!("cargo:rustc-link-arg-bins=-Tdefmt.x");
    Ok(())
}

/// The variable `name`, which the firmware cannot be built without.
fn required(name: &str) -> Result<String, Box<dyn Error>> {
    let Ok(value) = env::var(name) else {
        return Err(format!("the Pico 2 W firmware needs {name} set when it is built").into());
    };
    Ok(value)
}

/// The variable `name`, empty when it is unset, since every module variable may be.
fn optional(name: &str) -> String {
    env::var(name).unwrap_or_default()
}

/// The source of `board.rs`: the network to join, and the radio's blobs from `CYW43_FIRMWARE_DIR`.
#[expect(
    clippy::single_call_fn,
    reason = "the board generation step of main, kept apart from its linker directives"
)]
fn board() -> Result<String, Box<dyn Error>> {
    let firmware_dir = PathBuf::from(required("CYW43_FIRMWARE_DIR")?);
    let blob = |file: &str| -> Result<String, Box<dyn Error>> {
        let path = firmware_dir.join(file);
        if !path.is_file() {
            return Err(format!("CYW43_FIRMWARE_DIR holds no {file}").into());
        }
        Ok(format!(
            "include_bytes!({})",
            include_path("CYW43_FIRMWARE_DIR", &path)?
        ))
    };
    let network = required("WIFI_NETWORK")?;
    let password = required("WIFI_PASSWORD")?;
    let firmware = blob("43439A0.bin")?;
    let clm = blob("43439A0_clm.bin")?;
    let nvram = blob("nvram_rp2040.bin")?;
    Ok(format!(
        "/// The network the board joins.
const WIFI_NETWORK: &str = {network:?};
/// The network's passphrase.
const WIFI_PASSWORD: &str = {password:?};
/// The CYW43439's firmware.
static RADIO_FIRMWARE: &cyw43::Aligned<cyw43::A4, [u8]> = &cyw43::Aligned(*{firmware});
/// The CYW43439's country locale matrix.
static RADIO_CLM: &[u8] = {clm};
/// The CYW43439's configuration for the Pico 2 W's module.
static RADIO_NVRAM: &cyw43::Aligned<cyw43::A4, [u8]> = &cyw43::Aligned(*{nvram});
"
    ))
}
