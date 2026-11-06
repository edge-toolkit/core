//! A module's bytes as the stream `SpaceWasm` decodes it from: the whole module, as one chunk.

#![expect(
    unsafe_code,
    reason = "SpaceWasm takes each chunk as raw parts, which only unsafe code can build"
)]

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use spacewasm::{InnerVec, WasmStream};

/// The module's bytes, owned so that the one chunk handed out stays valid until the decoder hands it back.
pub(super) struct ModuleStream {
    /// The module, which the chunk points into.
    bytes: Vec<u8>,
    /// Whether the chunk has been handed out, after which the stream has ended.
    read: bool,
}

impl ModuleStream {
    /// A stream of a copy of `wasm`.
    #[expect(
        clippy::single_call_fn,
        reason = "the stream's constructor, called where the module is decoded"
    )]
    pub(super) fn new(wasm: &[u8]) -> Self {
        Self {
            bytes: wasm.to_vec(),
            read: false,
        }
    }
}

impl WasmStream for ModuleStream {
    /// The whole module the first time, and the end of the stream after that.
    fn read(&mut self) -> Result<Option<InnerVec<u8>>, u8> {
        if core::mem::replace(&mut self.read, true) {
            return Ok(None);
        }
        let len = self.bytes.len();
        // SAFETY: `bytes` is this stream's own allocation of `len` initialised bytes, which neither the decoder nor
        // `InnerVec` frees or writes, and which outlives the chunk: the decoder hands it back before the stream drops.
        Ok(Some(unsafe {
            InnerVec::from_raw_parts(self.bytes.as_mut_ptr(), len, len)
        }))
    }

    /// Nothing to reclaim: the chunk points into the stream's own bytes, which drop with it.
    fn return_(&mut self, _chunk: InnerVec<u8>) {}
}
