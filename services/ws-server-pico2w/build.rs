//! Puts the Pico 2 W's memory layout where the linker looks, links the firmware with cortex-m-rt's and defmt's linker
//! scripts, and generates `board.rs`: what the firmware bakes in, read here from the build's environment. On any
//! other target the binary is empty and needs none of it.

use std::env;
use std::error::Error;
use std::path::PathBuf;

/// The variables `board.rs` is generated from.
const BOARD_VARIABLES: [&str; 3] = ["CYW43_FIRMWARE_DIR", "WIFI_NETWORK", "WIFI_PASSWORD"];
/// The operating system cargo is building for, `none` for the board.
const CARGO_CFG_TARGET_OS: &str = "CARGO_CFG_TARGET_OS";
/// The directory cargo gives this build script for what it generates.
const OUT_DIR: &str = "OUT_DIR";

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=memory.x");
    for variable in BOARD_VARIABLES {
        println!("cargo:rerun-if-env-changed={variable}");
    }
    if env::var(CARGO_CFG_TARGET_OS)? != "none" {
        return Ok(());
    }
    let out_dir = PathBuf::from(env::var(OUT_DIR)?);
    fs_err::write(out_dir.join("memory.x"), include_bytes!("memory.x"))?;
    fs_err::write(out_dir.join("board.rs"), board()?)?;
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

/// The source of `board.rs`: the network to join, and the radio's blobs from `CYW43_FIRMWARE_DIR`.
#[expect(
    clippy::single_call_fn,
    reason = "the generation step of main, kept apart from its linker directives"
)]
fn board() -> Result<String, Box<dyn Error>> {
    let firmware_dir = PathBuf::from(required("CYW43_FIRMWARE_DIR")?);
    // A build script runs in its own crate's directory, so a relative path would quietly resolve against that.
    if !firmware_dir.is_absolute() {
        return Err("CYW43_FIRMWARE_DIR must be an absolute path".into());
    }
    let blob = |file: &str| -> Result<String, Box<dyn Error>> {
        let path = firmware_dir.join(file);
        if !path.is_file() {
            return Err(format!("CYW43_FIRMWARE_DIR holds no {file}").into());
        }
        let path = path.to_str().ok_or("CYW43_FIRMWARE_DIR is not valid UTF-8")?;
        Ok(format!("include_bytes!({path:?})"))
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
