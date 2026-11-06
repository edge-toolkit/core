//! Where `SpaceWasm`'s memory comes from: the global allocator, for its own structures and for a guest's linear memory.
//!
//! `SpaceWasm` allocates through two functions its embedder exports, rather than through the global allocator. These
//! forward to it, which every thread can share, so engines on different threads never contend for anything `SpaceWasm`
//! itself holds.

#![expect(
    unsafe_code,
    reason = "`SpaceWasm` allocates through exported functions and raw pointers, which only unsafe code can provide"
)]

use alloc::alloc::{alloc, dealloc, realloc};
use core::alloc::Layout;
use core::ptr::{NonNull, null_mut, without_provenance_mut};

use spacewasm::{AllocError, WasmMemoryAllocator};

/// `SpaceWasm`'s allocation hook: `size` bytes aligned to `align`, or null with the failure written to `err`.
///
/// A zero-sized allocation is a dangling pointer of that alignment, as `SpaceWasm` takes null for a failure.
///
/// # Safety
///
/// `err` must be valid to write a `u32` to, as `SpaceWasm` always hands it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __spacewasm_alloc(size: usize, align: usize, err: *mut u32) -> *mut u8 {
    let Ok(layout) = Layout::from_size_align(size, align) else {
        // SAFETY: the caller hands a pointer valid to write the failure to.
        unsafe {
            err.write(u32::from(AllocError::AllocationFailed));
        }
        return null_mut();
    };
    if size == 0 {
        return without_provenance_mut(align);
    }
    // SAFETY: `layout` has a non-zero size.
    let pointer = unsafe { alloc(layout) };
    if pointer.is_null() {
        // SAFETY: the caller hands a pointer valid to write the failure to.
        unsafe {
            err.write(u32::from(AllocError::OutOfMemory));
        }
    }
    pointer
}

/// `SpaceWasm`'s deallocation hook: free what [`__spacewasm_alloc`] gave it for `size` bytes aligned to `align`.
///
/// # Safety
///
/// `ptr` must be what `__spacewasm_alloc` returned for the same `size` and `align`, not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __spacewasm_dealloc(ptr: *mut u8, size: usize, align: usize) {
    if size == 0 {
        return;
    }
    if let Ok(layout) = Layout::from_size_align(size, align) {
        // SAFETY: the caller frees only what `__spacewasm_alloc` gave it for this layout, which was non-zero sized.
        unsafe { dealloc(ptr, layout) }
    }
}

/// A guest's linear memory, on the global allocator.
///
/// `SpaceWasm` zeroes each page it hands the guest, and never asks for a zero-sized memory: one of no pages is never
/// allocated, and a grow from it allocates rather than reallocating.
pub(super) struct LinearMemory;

impl WasmMemoryAllocator for LinearMemory {
    /// A new linear memory of `layout`, or why there is none.
    fn allocate(&self, layout: Layout) -> Result<NonNull<u8>, AllocError> {
        // SAFETY: `SpaceWasm` asks only for a memory of at least one page.
        NonNull::new(unsafe { alloc(layout) }).ok_or(AllocError::OutOfMemory)
    }

    /// The memory at `ptr` grown from `old_layout` to `layout`, its contents kept, or why it could not grow.
    fn reallocate(&self, ptr: NonNull<u8>, old_layout: Layout, layout: Layout) -> Result<NonNull<u8>, AllocError> {
        // SAFETY: `ptr` is this memory's, allocated here for `old_layout`; a grow keeps its alignment and is non-zero.
        NonNull::new(unsafe { realloc(ptr.as_ptr(), old_layout, layout.size()) }).ok_or(AllocError::OutOfMemory)
    }

    /// Free the memory at `ptr`, of `layout`.
    fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: `ptr` is this memory's, allocated here for `layout`.
        unsafe { dealloc(ptr.as_ptr(), layout) }
    }
}
