//! The `et` imports through which a Wasm 1.0 core module reaches its hub, and a typed guest API over them.
//!
//! A core module cannot import a WIT world, so it reaches the hub through the operations of `et:ws-wasi` flattened
//! into the import module `et`, every parameter and result an `i32`. A pointer and a length name bytes in the guest's
//! linear memory, and every message crosses as the JSON of an `edge_toolkit::ws` message, the hub's own wire format.
//! A call that produces bytes leaves them in the host's result slot, and `take` copies them out. `et-ws-wasi-runner`
//! hosts these imports for a core module, which it runs on NASA's `SpaceWasm` interpreter rather than wasmtime.
//!
//! | Import       | Parameters                                        | Result                                      |
//! | ------------ | ------------------------------------------------- | ------------------------------------------- |
//! | `connect`    | none                                              | [`abi::SUCCESS`] once registered            |
//! | `agent_id`   | none                                              | the id's length, the id in the slot         |
//! | `send`       | `ClientMessage` JSON: pointer, length             | [`abi::SUCCESS`]                            |
//! | `recv`       | timeout in milliseconds                           | a `ServerMessage`'s length, in the slot     |
//! | `take`       | buffer: pointer, capacity                         | the slot's length, copied out if it fits    |
//! | `disconnect` | none                                              | none                                        |
//! | `kv_get`     | bucket: pointer, length; key: the same            | [`abi::FOUND`], the value in the slot       |
//! | `kv_set`     | bucket, key, value: each pointer, length          | [`abi::SUCCESS`]                            |
//! | `log`        | level 0-5; context, message: each pointer, length | none                                        |
//!
//! `recv` returns [`abi::NOTHING`] when no message arrived in time, `kv_get` returns [`abi::ABSENT`] for a key the
//! bucket holds nothing under, and `take` empties the slot once it has copied it out. A failed call returns one of the
//! negative codes in [`abi`], and the host logs why. A bad pointer, text that is not UTF-8 or an unknown level traps
//! the guest. The guest exports [`abi::RUN`], taking nothing and returning an `i32`, which is [`abi::SUCCESS`] when it
//! succeeded.
//!
//! [`Guest`] is the typed API, over any [`Imports`]. Built for a Wasm 1.0 target, `wasm32v1-none`, the crate also
//! provides the guest's imports, its heap and its panic handler, and its `export!` macro defines the guest's `run`.

#![no_std]

extern crate alloc;

pub mod abi;
mod guest;
#[cfg(all(target_arch = "wasm32", target_os = "none"))]
pub mod wasm;

pub use self::guest::{Guest, GuestError, Imports, Level};
