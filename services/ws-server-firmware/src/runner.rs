//! What the wasi runners the firmware starts share: the guests the build built in, the memory they draw on, and the
//! hub's storage when the hub is beside them.
//!
//! Each runs through `et_ws_wasi_runner::board::run_guest`, reaching its hub through the transport it is handed: the
//! hub beside it, or one on another machine. The board and the host share all of it but the clock and the log, which
//! each hands in as a platform.

use core::ptr::null_mut;
use core::sync::atomic::{AtomicPtr, Ordering};

#[cfg(not(remote_hub))]
use et_ws_service::picoserve_hub::Hub;
#[cfg(not(remote_hub))]
use et_ws_wasi_runner::board::Buckets;
use et_ws_wasi_runner::board::MemoryBudget;

#[cfg(not(remote_hub))]
use crate::local::{BoardStorage, HubTransport};

include!(concat!(env!("OUT_DIR"), "/guests.rs"));

/// How many guest tasks a board's executor holds room for.
///
/// One per guest, and one for a build with none, since a pool cannot be empty.
#[cfg_attr(
    not(target_os = "none"),
    expect(dead_code, reason = "the host spawns its guests without a pool")
)]
pub(crate) const GUEST_POOL: usize = if GUESTS.is_empty() { 1 } else { GUESTS.len() };

/// The pointers wasmtime keeps for the call it is in, which with one thread are statics rather than a thread's.
///
/// Slot 0 is its runtime state, and slot 1 its component-model-async state; it asks for no other.
static WASMTIME_TLS: [AtomicPtr<u8>; 2] = [AtomicPtr::new(null_mut()), AtomicPtr::new(null_mut())];

/// wasmtime's read of thread-local `slot`, which a build without `std` asks the embedder for.
#[expect(
    unsafe_code,
    reason = "wasmtime links against this name, so it has to be exported unmangled; nothing else defines it"
)]
#[unsafe(no_mangle)]
extern "C" fn wasmtime_tls_get(slot: usize) -> *mut u8 {
    WASMTIME_TLS
        .get(slot)
        .map_or(null_mut(), |pointer| pointer.load(Ordering::Relaxed))
}

/// wasmtime's write of thread-local `slot`, which a build without `std` asks the embedder for.
#[expect(
    unsafe_code,
    reason = "wasmtime links against this name, so it has to be exported unmangled; nothing else defines it"
)]
#[unsafe(no_mangle)]
extern "C" fn wasmtime_tls_set(slot: usize, pointer: *mut u8) {
    if let Some(stored) = WASMTIME_TLS.get(slot) {
        stored.store(pointer, Ordering::Relaxed);
    }
}

/// What every guest on the board shares: the storage of a hub beside them, and the memory the build holds for them.
#[derive(Clone)]
pub(crate) struct Shared {
    /// The storage of the hub beside the guests.
    #[cfg(not(remote_hub))]
    buckets: Buckets,
    /// The memory the build holds for the guests' memories and tables.
    memory: MemoryBudget,
}

impl Shared {
    /// The board's storage, empty, and all of [`GUEST_MEMORY`], none of it drawn on yet.
    #[expect(
        clippy::single_call_fn,
        reason = "made once, where the board's guests are started, as every guest has to share the one"
    )]
    pub(crate) fn new() -> Self {
        Self {
            #[cfg(not(remote_hub))]
            buckets: Buckets::default(),
            memory: MemoryBudget::new(usize::try_from(GUEST_MEMORY).unwrap_or(usize::MAX)),
        }
    }

    /// The memory the build holds for the guests, which each guest draws on.
    pub(crate) fn memory(&self) -> MemoryBudget {
        self.memory.clone()
    }

    /// The guests' storage, for the hub beside them to serve.
    #[cfg(not(remote_hub))]
    pub(crate) fn storage(&self) -> BoardStorage {
        BoardStorage::new(self.buckets.clone())
    }

    /// A guest's transport into `hub`, its storage the board's, waiting on `platform`'s clock, not yet open.
    #[cfg(not(remote_hub))]
    pub(crate) fn hub_transport<S, P>(&self, hub: &'static Hub<S>, platform: P) -> HubTransport<S, P> {
        HubTransport::new(hub, platform, self.buckets.clone())
    }
}
