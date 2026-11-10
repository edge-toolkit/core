//! Decoders for the serial streams of 2D lidars, turning their bytes into whole revolutions of samples.
//!
//! Each lidar has its own module. A decoder does no I/O: whatever reads the port hands it the bytes as they arrive,
//! in chunks of any size, so the same decoder serves a serial port, a browser's Web Serial stream and a board's UART.
//! `no_std` with `alloc` for that last one.

#![no_std]

extern crate alloc;

pub mod coin_d6;
