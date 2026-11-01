//! The firmware on a board: each board's module brings up its radio and joins the network named at build time, and
//! [`hub`] serves the hub on it.

#[cfg(target_arch = "riscv32")]
mod esp32c6;
mod hub;
#[cfg(target_arch = "arm")]
mod pico2w;
