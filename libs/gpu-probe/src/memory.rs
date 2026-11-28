//! How much memory a GPU has: what the graphics API beneath wgpu says, and how far allocating goes before it refuses.
//!
//! The two answer different questions. On a discrete GPU the API names the memory on the card, and allocating stops
//! short of it by what the driver and the display hold: an RTX A1000 Laptop GPU's Vulkan heap is 4096 MiB, its budget
//! 3761 MiB, and allocating stopped at 3440 MiB. On unified memory -- Apple silicon, NVIDIA's GB10, an integrated GPU
//! -- the API names the share of the system's memory the GPU may take, and allocating reaches it: a GB10's heap is
//! 93456 MiB, and allocating stopped at 93296 MiB. The walk is made of WebGPU calls alone, so it runs in a page too,
//! where nothing beneath WebGPU can be reached.
#![expect(
    clippy::single_call_fn,
    reason = "reading each API, and allocating one buffer of the walk, are named steps of their own"
)]
#![cfg_attr(
    target_arch = "wasm32",
    expect(
        clippy::future_not_send,
        reason = "a page's WebGPU objects are JavaScript's, which stay on the one thread a page has"
    )
)]

use std::sync::atomic::{AtomicBool, Ordering};

use crate::native::{NativeError, SUBMISSION_TIMEOUT, backend_name};
#[cfg(not(target_arch = "wasm32"))]
use crate::r#unsafe;

/// One mebibyte.
const MIB: u64 = 1 << 20_u32;

/// The smallest buffer the walk tries, so it stops within this many bytes of where the device refuses.
pub const WALK_RESOLUTION: u64 = 16 * MIB;

/// What the graphics API beneath wgpu says of a GPU's memory, in bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ApiMemory {
    /// The API that said so: `vulkan`, `metal` or `dxgi`.
    pub api: &'static str,
    /// The memory the GPU holds as its own.
    ///
    /// That is Vulkan's device-local heaps, DXGI's dedicated video memory, or Metal's recommended working set, which on
    /// unified memory is the share of the system's memory Metal recommends the GPU keeps to.
    pub local: u64,
    /// How much of `local` the API says may be used.
    ///
    /// That is Vulkan's budget, where the driver gives one, and otherwise all of it.
    pub budget: u64,
    /// Whether the GPU's memory is the system's own, which the CPU uses too.
    ///
    /// That is where every Vulkan heap is device-local, where Metal says so, and for DXGI an adapter that is not
    /// discrete.
    pub unified: bool,
}

/// The graphics API beneath wgpu gave no memory size, because the device was opened through one this does not read.
///
/// That is OpenGL, a page's WebGPU, and wgpu's no-op backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{backend} gives no memory size beneath wgpu")]
#[non_exhaustive]
pub struct NoApiMemory {
    /// The registry's name for the backend the device was opened through.
    pub backend: &'static str,
}

/// One of the memory heaps a Vulkan device has.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct VulkanHeap {
    /// The heap's size.
    pub size: u64,
    /// The budget `VK_EXT_memory_budget` gives the heap, or 0 where the driver gives none.
    pub budget: u64,
    /// Whether the heap is device-local.
    pub device_local: bool,
}

impl VulkanHeap {
    /// A heap of `size` bytes, budgeted at `budget`, device-local or not.
    #[must_use]
    pub const fn new(size: u64, budget: u64, device_local: bool) -> Self {
        Self {
            size,
            budget,
            device_local,
        }
    }
}

/// What Vulkan's `heaps` say of the GPU's memory.
///
/// The device-local heaps are the GPU's own; a heap with no budget is budgeted at its size. The memory is unified where
/// no heap is the system's alone, as on an integrated GPU: a discrete GPU has its own heap beside the system's.
#[must_use]
pub fn vulkan_memory(heaps: &[VulkanHeap]) -> ApiMemory {
    let local_heaps = heaps.iter().filter(|heap| heap.device_local);
    let budget_of = |heap: &VulkanHeap| if heap.budget == 0 { heap.size } else { heap.budget };
    ApiMemory {
        api: "vulkan",
        local: local_heaps.clone().map(|heap| heap.size).fold(0, u64::saturating_add),
        budget: local_heaps.map(budget_of).fold(0, u64::saturating_add),
        unified: heaps.iter().all(|heap| heap.device_local),
    }
}

/// What the graphics API beneath `adapter` and `device` says of the GPU's memory.
///
/// Vulkan is read from the adapter, Metal from the device, and DXGI from the adapter beneath Direct3D 12.
pub(crate) fn api_memory(
    adapter: &wgpu::Adapter,
    #[cfg_attr(
        not(target_vendor = "apple"),
        expect(unused_variables, reason = "only Metal is read from the device")
    )]
    device: &wgpu::Device,
) -> Result<ApiMemory, NoApiMemory> {
    #[cfg(not(any(target_vendor = "apple", target_arch = "wasm32")))]
    if let Ok(heaps) = r#unsafe::vulkan_heaps(adapter) {
        return Ok(vulkan_memory(&heaps));
    }
    #[cfg(target_vendor = "apple")]
    if let Ok(memory) = r#unsafe::with_metal(device, |metal| metal_memory(metal.raw_device())) {
        return Ok(memory);
    }
    #[cfg(windows)]
    if let Ok((Ok(description), _)) = r#unsafe::dxgi_adapter(adapter) {
        let local = u64::try_from(description.DedicatedVideoMemory).unwrap_or(u64::MAX);
        return Ok(ApiMemory {
            api: "dxgi",
            local,
            budget: local,
            unified: adapter.get_info().device_type != wgpu::DeviceType::DiscreteGpu,
        });
    }
    let info = adapter.get_info();
    Err(NoApiMemory {
        backend: backend_name(info.backend, &info.driver_info),
    })
}

/// What the Metal device `raw` says of its memory.
#[cfg(target_vendor = "apple")]
fn metal_memory(raw: &objc2::runtime::ProtocolObject<dyn objc2_metal::MTLDevice>) -> ApiMemory {
    use objc2_metal::MTLDevice as _;
    let working_set = raw.recommendedMaxWorkingSetSize();
    ApiMemory {
        api: "metal",
        local: working_set,
        budget: working_set,
        unified: raw.hasUnifiedMemory(),
    }
}

/// How far allocating went before the device refused, or before the walk reached the most it was allowed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Walk {
    /// The bytes allocated, and held at once, when the walk stopped.
    pub bytes: u64,
    /// Whether the device refused: false where the walk stopped at its cap instead.
    pub refused: bool,
}

/// How far `allocate` goes before it refuses, holding everything it gives until the walk ends.
///
/// It is asked for buffers of `largest` bytes, then, each time it refuses, half that, down to [`WALK_RESOLUTION`]; it
/// is never asked for more than `cap` bytes in all, which keeps a walk on unified memory from pushing the system into
/// swap. `allocate` answers with what it holds, its refusal, or a failure that ends the walk.
pub async fn walk_with<Allocate, Pending, Held, Refusal, Failure>(
    cap: u64,
    largest: u64,
    allocate: Allocate,
) -> Result<Walk, Failure>
where
    Allocate: Fn(u64) -> Pending,
    Pending: Future<Output = Result<Result<Held, Refusal>, Failure>>,
{
    // Each buffer held, beside its size: the walk holds every one until it ends, which is what makes the next refuse.
    let mut held: Vec<(Held, u64)> = Vec::new();
    let mut chunk = largest.max(WALK_RESOLUTION);
    loop {
        let bytes = held.iter().map(|(_, size)| *size).fold(0, u64::saturating_add);
        // The walk asks only for whole multiples of its resolution, which every buffer size it tries is.
        let room = cap.saturating_sub(bytes) & !(WALK_RESOLUTION - 1);
        if room == 0 {
            return Ok(Walk { bytes, refused: false });
        }
        let size = chunk.min(room);
        match allocate(size).await? {
            Ok(buffer) => held.push((buffer, size)),
            Err(_refused) if size <= WALK_RESOLUTION => return Ok(Walk { bytes, refused: true }),
            Err(_refused) => chunk = (size >> 1_u32).max(WALK_RESOLUTION),
        }
    }
}

/// A buffer a walk holds, destroyed as it is dropped.
///
/// In a page, dropping a `wgpu::Buffer` frees none of its memory, which waits for the browser's garbage collector, so
/// a walk that ended without destroying what it held would leave the next walk on the page refused that much sooner.
/// Destroying it here covers every way a walk ends: reaching its cap, being refused, failing, and being abandoned.
pub(crate) struct WalkBuffer(wgpu::Buffer);

impl Drop for WalkBuffer {
    fn drop(&mut self) {
        self.0.destroy();
    }
}

/// A buffer of `size` bytes on `device`, cleared so that it is backed by memory, or the device's refusal.
///
/// The device refuses a buffer as it is created, or runs out of memory creating the encoder, the clear or the
/// submission after it, so the scope catching that spans every step. A refused buffer is invalid, and clearing it is
/// a validation error that follows from the refusal alone, so the clear's error, caught by a scope nested inside it,
/// is taken as the refusal only where running out of memory gave none. Both scopes are popped, innermost first, before
/// either is awaited, so neither guard, which is not `Send`, is held across an `await`.
///
/// The clear is waited for until the device says it is done, so the memory is backed before the next buffer is asked
/// for. Natively the poll runs that callback before it returns; in a page the poll does nothing, the browser runs the
/// callback later, and the wait is on the channel it answers through. Without that wait headless Chromium on a GB10
/// said it held 131072 MiB within 50 ms: all 128 GiB the machine has, where its Vulkan heap is 93456 MiB.
///
/// A device lost meanwhile ends the walk. Natively wgpu drops the callback uncalled; a page's WebGPU calls it anyway
/// and its error scopes come back empty, so there the loss is read from `lost`, which the device's lost callback sets,
/// and a buffer is never counted for a device that cannot hold it.
pub(crate) async fn allocate(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    lost: &AtomicBool,
    size: u64,
) -> Result<Result<WalkBuffer, wgpu::Error>, NativeError> {
    let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("gpu-probe memory walk"),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.clear_buffer(&buffer, 0, None);
    let _submission: wgpu::SubmissionIndex = queue.submit([encoder.finish()]);
    let (done, finished) = futures_channel::oneshot::channel::<()>();
    queue.on_submitted_work_done(move || {
        // The receiver is awaited below, so it is still there to take this unless the whole walk was abandoned.
        let _sent: Result<(), ()> = done.send(());
    });
    // Every submission is waited for, not this one: a refused buffer's clear is never submitted, and waiting for its
    // index failed on an RTX A1000 Laptop GPU under NVIDIA's 595.104.02 Vulkan driver with WrongSubmissionIndex(14,
    // 13).
    let _status: wgpu::PollStatus = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: Some(SUBMISSION_TIMEOUT),
    })?;
    // Popped, though not yet awaited, before the wait, so neither guard is held across it; each scope still holds the
    // errors that arose as the clear was submitted.
    let invalid = validation.pop();
    let refused = memory.pop();
    // No GPU a test runs on is lost, so each sign of a loss is taken as a value rather than tested, which would leave a
    // branch no run could take. `lost` is read last, so a loss reported while the scopes were awaited is seen.
    finished.await.or(Err(NativeError::Lost))?;
    let (invalid, refused) = (invalid.await, refused.await);
    (!lost.load(Ordering::Relaxed)).then_some(()).ok_or(NativeError::Lost)?;
    Ok(refused.or(invalid).map_or(Ok(WalkBuffer(buffer)), Err))
}
