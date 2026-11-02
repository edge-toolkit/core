//! The firmware on a board: each board's module brings up its radio and joins the network named at build time, and
//! [`hub`] serves the hub on it.

#[cfg(all(target_arch = "arm", not(any(feature = "pico-plus2w", feature = "pico2w"))))]
compile_error!("a bare-metal Arm build is for a board: enable the `pico2w` or the `pico-plus2w` feature");

#[cfg(target_arch = "riscv32")]
mod esp32c6;
mod hub;
#[cfg(target_arch = "arm")]
mod pico2w;
