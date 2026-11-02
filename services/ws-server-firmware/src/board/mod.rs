//! The firmware on a board: each board's module brings up its radio and joins the network named at build time, and
//! its role serves on it: [`hub`] the hub and the guests beside it, or for a build with a hub elsewhere, `remote` the
//! guests alone, reaching that hub.

#[cfg(all(target_arch = "arm", not(any(feature = "pico-plus2w", feature = "pico2w"))))]
compile_error!("a bare-metal Arm build is for a board: enable the `pico2w` or the `pico-plus2w` feature");

#[cfg(target_arch = "riscv32")]
mod esp32c6;
#[cfg(not(remote_hub))]
mod hub;
#[cfg(target_arch = "arm")]
mod pico2w;
mod platform;
#[cfg(remote_hub)]
mod remote;

#[cfg(not(remote_hub))]
use self::hub as role;
#[cfg(remote_hub)]
use self::remote as role;
