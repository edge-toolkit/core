//! Covers what a board's build writes and links with: each board's files and directives, and every way a build for
//! one is refused.
#![cfg(test)]

use std::collections::BTreeMap;
use std::env::VarError;
use std::path::Path;

use et_firmware_modules::{BOARD_VARIABLES, ModulesError, board};
use fs_err as fs;
use tempfile::{TempDir, tempdir};

/// The layout handed over as the Pico 2 W's `memory.x`.
const MEMORY_X: &[u8] = b"MEMORY {}";

/// The radio's blobs, each with something in it, in a directory of their own.
fn blobs() -> TempDir {
    let dir = tempdir().unwrap();
    for file in ["43439A0.bin", "43439A0_clm.bin", "nvram_rp2040.bin"] {
        fs::write(dir.path().join(file), file).unwrap();
    }
    dir
}

/// What `board` returns for `arch`, reading only `vars`, and the directory it wrote to.
fn built(arch: &str, vars: &[(&str, &str)]) -> (TempDir, Result<Vec<String>, ModulesError>) {
    let vars: BTreeMap<String, String> = vars
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect();
    let out_dir = tempdir().unwrap();
    let var = |name: &str| vars.get(name).cloned().ok_or(VarError::NotPresent);
    let directives = board(arch, out_dir.path(), MEMORY_X, var);
    (out_dir, directives)
}

/// The file `name` that `board` wrote to `out_dir`.
fn written(out_dir: &Path, name: &str) -> String {
    fs::read_to_string(out_dir.join(name)).unwrap()
}

#[test]
fn an_esp32c6_joins_the_network_and_links_with_esp_hal() {
    let (out_dir, directives) = built(
        "riscv32",
        &[("WIFI_NETWORK", "home"), ("WIFI_PASSWORD", "pass \"word\"")],
    );
    assert_eq!(
        directives.unwrap(),
        [
            "cargo:rustc-link-arg-bins=-Tlinkall.x",
            "cargo:rustc-link-arg-bins=-Tdefmt.x"
        ]
    );
    let wifi = written(out_dir.path(), "wifi.rs");
    assert!(wifi.contains(r#"const WIFI_NETWORK: &str = "home";"#));
    assert!(wifi.contains(r#"const WIFI_PASSWORD: &str = "pass \"word\"";"#));
    assert!(!out_dir.path().join("cyw43.rs").exists());
    assert!(!out_dir.path().join("memory.x").exists());
}

#[test]
fn a_pico_2_w_also_bakes_in_its_radio_and_its_memory_layout() {
    let blobs = blobs();
    let dir = blobs.path().to_str().unwrap();
    let (out_dir, directives) = built(
        "arm",
        &[
            ("WIFI_NETWORK", "home"),
            ("WIFI_PASSWORD", "pass"),
            ("CYW43_FIRMWARE_DIR", dir),
        ],
    );
    assert_eq!(
        directives.unwrap(),
        [
            format!("cargo:rustc-link-search={}", out_dir.path().display()),
            "cargo:rustc-link-arg-bins=--nmagic".to_owned(),
            "cargo:rustc-link-arg-bins=-Tlink.x".to_owned(),
            "cargo:rustc-link-arg-bins=-Tdefmt.x".to_owned(),
        ]
    );
    assert_eq!(fs::read(out_dir.path().join("memory.x")).unwrap(), MEMORY_X);
    let radio = written(out_dir.path(), "cyw43.rs");
    for file in ["43439A0.bin", "43439A0_clm.bin", "nvram_rp2040.bin"] {
        let literal = format!("{:?}", blobs.path().join(file).to_str().unwrap());
        assert!(radio.contains(&format!("include_bytes!({literal})")));
    }
    assert!(written(out_dir.path(), "wifi.rs").contains(r#"const WIFI_NETWORK: &str = "home";"#));
    assert!(BOARD_VARIABLES.contains(&"CYW43_FIRMWARE_DIR"));
}

#[test]
fn a_build_for_no_board_or_without_what_its_board_needs_is_refused() {
    let blobs = blobs();
    let dir = blobs.path().to_str().unwrap();
    let wifi = [("WIFI_NETWORK", "home"), ("WIFI_PASSWORD", "pass")];
    let (_out, no_board) = built("x86_64", &wifi);
    assert_eq!(no_board.unwrap_err().to_string(), "no board is built for x86_64");
    let (_out, no_password) = built("riscv32", &[("WIFI_NETWORK", "home")]);
    assert_eq!(
        no_password.unwrap_err().to_string(),
        "a board's firmware needs WIFI_PASSWORD set when it is built"
    );
    let (_out, no_blobs) = built("arm", &wifi);
    assert_eq!(
        no_blobs.unwrap_err().to_string(),
        "a board's firmware needs CYW43_FIRMWARE_DIR set when it is built"
    );
    let (_out, relative) = built("arm", &[wifi[0], wifi[1], ("CYW43_FIRMWARE_DIR", "relative")]);
    assert!(matches!(relative.unwrap_err(), ModulesError::MissingBlob { .. }));

    fs::remove_file(blobs.path().join("43439A0_clm.bin")).unwrap();
    let (_out, missing) = built("arm", &[wifi[0], wifi[1], ("CYW43_FIRMWARE_DIR", dir)]);
    assert_eq!(
        missing.unwrap_err().to_string(),
        "CYW43_FIRMWARE_DIR holds no 43439A0_clm.bin"
    );
}
