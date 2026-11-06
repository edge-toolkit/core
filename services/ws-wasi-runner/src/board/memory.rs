//! What a board holds for its guests' memories, and each guest's draw on it as its memories and tables grow.

use alloc::sync::Arc;
#[cfg(feature = "core-modules")]
use core::alloc::Layout;
#[cfg(feature = "core-modules")]
use core::ptr::NonNull;
use core::sync::atomic::{AtomicUsize, Ordering};

#[cfg(feature = "core-modules")]
use spacewasm::{AllocError, WasmMemoryAllocator};
use wasmtime::ResourceLimiter;

#[cfg(feature = "core-modules")]
use crate::core_module::LinearMemory;

/// The bytes a board holds for its guests' memories and tables, which every guest it runs draws on as they grow.
///
/// A handle: its clones are the same bytes, so every guest a board runs is handed one. A guest whose growth would spend
/// more than is left is refused it, so its `memory.grow` fails in the guest rather than the board running out of heap,
/// and a guest starting with more than is left fails to instantiate. What a guest held is returned when it ends.
#[derive(Clone, Debug)]
pub struct MemoryBudget(Arc<AtomicUsize>);

impl MemoryBudget {
    /// A budget of `bytes`, none of it yet drawn on.
    #[must_use]
    pub fn new(bytes: usize) -> Self {
        Self(Arc::new(AtomicUsize::new(bytes)))
    }

    /// The bytes not yet drawn on.
    #[must_use]
    pub fn left(&self) -> usize {
        self.0.load(Ordering::Acquire)
    }

    /// Draw `bytes` from the budget, if that many are left.
    fn take(&self, bytes: usize) -> bool {
        self.0
            .try_update(Ordering::AcqRel, Ordering::Acquire, |left| left.checked_sub(bytes))
            .is_ok()
    }

    /// Return `bytes` drawn earlier.
    fn give_back(&self, bytes: usize) {
        let _before = self.0.fetch_add(bytes, Ordering::AcqRel);
    }
}

/// One guest's draw on its board's [`MemoryBudget`]: what it holds, and what its latest growth took.
pub(super) struct Allowance {
    budget: MemoryBudget,
    /// Every byte this guest has drawn and not returned.
    held: usize,
    /// What the latest growth drew, returned if wasmtime then fails to grow after all.
    latest: usize,
}

impl Allowance {
    /// A guest's draw on `budget`, holding nothing yet.
    #[expect(
        clippy::single_call_fn,
        reason = "the allowance's constructor, called where each guest's store data is made"
    )]
    pub(super) const fn new(budget: MemoryBudget) -> Self {
        Self {
            budget,
            held: 0,
            latest: 0,
        }
    }

    /// Draw what growing from `current` bytes to `desired` takes, and whether the budget had it.
    fn grow(&mut self, current: usize, desired: usize) -> bool {
        let wanted = desired.saturating_sub(current);
        if !self.budget.take(wanted) {
            return false;
        }
        self.held = self.held.saturating_add(wanted);
        self.latest = wanted;
        true
    }

    /// Return what the latest growth drew, since it did not happen.
    fn refund_latest(&mut self) {
        self.budget.give_back(self.latest);
        self.held = self.held.saturating_sub(self.latest);
        self.latest = 0;
    }
}

impl ResourceLimiter for Allowance {
    /// A memory's growth, or its first allocation at instantiation, allowed only if the budget has it.
    fn memory_growing(&mut self, current: usize, desired: usize, _maximum: Option<usize>) -> wasmtime::Result<bool> {
        Ok(self.grow(current, desired))
    }

    /// Return what a memory's allowed growth drew, since wasmtime could not grow it after all.
    fn memory_grow_failed(&mut self, _error: wasmtime::Error) -> wasmtime::Result<()> {
        self.refund_latest();
        Ok(())
    }

    /// A table's growth, counted at the pointer wasmtime keeps for each element.
    fn table_growing(&mut self, current: usize, desired: usize, _maximum: Option<usize>) -> wasmtime::Result<bool> {
        let pointer = size_of::<usize>();
        Ok(self.grow(current.saturating_mul(pointer), desired.saturating_mul(pointer)))
    }

    /// Return what a table's allowed growth drew, since wasmtime could not grow it after all.
    fn table_grow_failed(&mut self, _error: wasmtime::Error) -> wasmtime::Result<()> {
        self.refund_latest();
        Ok(())
    }
}

impl Drop for Allowance {
    /// Return everything this guest held, as it ends.
    fn drop(&mut self) {
        self.budget.give_back(self.held);
    }
}

/// A core module's linear memory, drawn from its board's [`MemoryBudget`] as it is allocated and as it grows.
///
/// A module starting with more than is left fails to load, and growth past what is left fails in the guest, as
/// `memory.grow` returning -1. What the memory held is returned when `SpaceWasm` frees it, as the module ends.
#[cfg(feature = "core-modules")]
pub(super) struct CoreMemory(MemoryBudget);

#[cfg(feature = "core-modules")]
impl CoreMemory {
    /// A core module's memory, drawing on `budget`.
    #[expect(
        clippy::single_call_fn,
        reason = "the memory's constructor, called where a core module is run"
    )]
    pub(super) const fn new(budget: MemoryBudget) -> Self {
        Self(budget)
    }

    /// What `allocate` returns, once `bytes` are drawn from the budget; nothing stays drawn when either fails.
    fn drawing<A>(&self, bytes: usize, allocate: A) -> Result<NonNull<u8>, AllocError>
    where
        A: FnOnce() -> Result<NonNull<u8>, AllocError>,
    {
        if !self.0.take(bytes) {
            return Err(AllocError::OutOfMemory);
        }
        // Only the board's heap running out fails `allocate` after the budget allowed it, which a test cannot cause.
        allocate().inspect_err(|_| self.0.give_back(bytes))
    }
}

#[cfg(feature = "core-modules")]
impl WasmMemoryAllocator for CoreMemory {
    /// A new memory of `layout`, drawing its whole size.
    fn allocate(&self, layout: Layout) -> Result<NonNull<u8>, AllocError> {
        self.drawing(layout.size(), || LinearMemory.allocate(layout))
    }

    /// The memory at `ptr` grown from `old_layout` to `layout`, drawing what it grew by.
    fn reallocate(&self, ptr: NonNull<u8>, old_layout: Layout, layout: Layout) -> Result<NonNull<u8>, AllocError> {
        let grown = layout.size().saturating_sub(old_layout.size());
        self.drawing(grown, || LinearMemory.reallocate(ptr, old_layout, layout))
    }

    /// Free the memory at `ptr`, returning its whole size.
    fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        LinearMemory.deallocate(ptr, layout);
        self.0.give_back(layout.size());
    }
}
