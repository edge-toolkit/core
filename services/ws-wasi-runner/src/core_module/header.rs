//! Telling a core module from other bytes by its header.

/// The first bytes of every Wasm 1.0 core module: WebAssembly's magic number, then version 1.
const HEADER: [u8; 8] = *b"\0asm\x01\0\0\0";

/// Whether `bytes` are a core module, as their header says.
///
/// A component's header carries another version, and what wasmtime compiles a component to is no WebAssembly at all, so
/// this tells a core module from either.
#[must_use]
pub fn is_core_module(bytes: &[u8]) -> bool {
    bytes.starts_with(&HEADER)
}
