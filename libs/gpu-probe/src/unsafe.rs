//! Every unsafe call the crate makes, each behind a safe function that says why the call is sound.
//!
//! wgpu reaches the graphics API beneath it, and offers its experimental features, only through unsafe calls, and the
//! APIs themselves -- OpenGL through glow, Metal through objc2, DXGI through windows -- are unsafe to call. Keeping
//! every such call here leaves the rest of the crate free of `unsafe`, and puts each call beside its reasoning.
#![expect(
    clippy::single_call_fn,
    unsafe_code,
    reason = "this module is where the crate's unsafe calls are kept, each wrapped once beside what makes it sound"
)]

#[cfg(windows)]
use windows::core::Interface as _;

/// An adapter or device was opened through another graphics API than the one a call reads beneath it.
///
/// It is no failure of the device: wgpu opens each adapter through one API, and the others have nothing beneath it.
///
/// A page reaches no API beneath WebGPU, so there it is never made.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub(crate) struct OtherApi;

/// The experimental features every device is opened with, which lets a device be asked for cooperative matrices.
///
/// Letting it asks for nothing: a device gets an experimental feature only when it is named among the required ones,
/// which cooperative matrices are only where a matrix probe would run on them. So every device is opened with this,
/// rather than a branch choosing between it and none.
pub(crate) const fn experimental_features() -> wgpu::ExperimentalFeatures {
    // SAFETY: the one experimental feature ever asked for is cooperative matrices, which only the matrix probes use;
    // each runs in a shader of its own, and a probe the device mishandles answers with its refusal.
    unsafe { wgpu::ExperimentalFeatures::enabled() }
}

/// What `read` makes of the Vulkan adapter beneath `adapter`.
///
/// wgpu builds its Vulkan backend everywhere but on Apple's platforms and in a page.
#[cfg(not(any(target_vendor = "apple", target_arch = "wasm32")))]
pub(crate) fn with_vulkan<Read, Out>(adapter: &wgpu::Adapter, read: Read) -> Result<Out, OtherApi>
where
    Read: FnOnce(&wgpu::hal::vulkan::Adapter) -> Out,
{
    // SAFETY: the Vulkan adapter is only read from, by `read`, and is not kept past this function.
    let hal = unsafe { adapter.as_hal::<wgpu::hal::api::Vulkan>() };
    hal.map(|vulkan| read(&vulkan)).ok_or(OtherApi)
}

/// The memory heaps of the Vulkan device beneath `adapter`, each with its budget where the driver gives one.
///
/// Budgets come from `VK_EXT_memory_budget`, chained only where the device offers it, as Vulkan requires. Without it
/// every budget is left at 0, which `vulkan_memory` budgets at the heap's size.
///
/// The heaps are read through `VK_KHR_get_physical_device_properties2`, which wgpu enables wherever the driver offers
/// it, rather than the entry point Vulkan 1.1 promoted it to: a Vulkan 1.0 instance can offer the extension yet lack
/// the core entry point, which ash then answers by panicking. An instance without the extension gives no heaps here, as
/// one that is not Vulkan does.
#[cfg(not(any(target_vendor = "apple", target_arch = "wasm32")))]
pub(crate) fn vulkan_heaps(adapter: &wgpu::Adapter) -> Result<Vec<crate::memory::VulkanHeap>, OtherApi> {
    use ash::khr::get_physical_device_properties2 as properties2;
    use ash::vk;
    with_vulkan(adapter, |vulkan| {
        let shared = vulkan.shared_instance();
        let instance = shared.raw_instance();
        let physical = vulkan.raw_physical_device();
        // Every driver a test runs on offers both extensions, so each absence is taken as a value, through `Option`,
        // rather than by an `if` that would leave a branch no run could take.
        let enabled = shared.extensions().contains(&properties2::NAME);
        let loaded = enabled
            .then(|| properties2::Instance::new(shared.entry(), instance))
            .ok_or(OtherApi)?;
        // SAFETY: the call only lists what the physical device offers, which the instance outlives.
        let extensions = unsafe { instance.enumerate_device_extension_properties(physical) }.unwrap_or_default();
        let budgeted = extensions
            .iter()
            .any(|extension| extension.extension_name_as_c_str() == Ok(vk::EXT_MEMORY_BUDGET_NAME));
        let mut budget = vk::PhysicalDeviceMemoryBudgetPropertiesEXT::default();
        #[expect(
            clippy::obfuscated_if_else,
            reason = "an `if` here is a branch no test driver can take, since every one offers VK_EXT_memory_budget"
        )]
        let mut properties = budgeted
            .then(|| vk::PhysicalDeviceMemoryProperties2::default().push_next(&mut budget))
            .unwrap_or_default();
        // SAFETY: the call only fills in `properties`, and the budget chained to it, both of which outlive it.
        unsafe {
            loaded.get_physical_device_memory_properties2(physical, &mut properties);
        }
        let memory = properties.memory_properties;
        Ok(memory
            .memory_heaps_as_slice()
            .iter()
            .zip(budget.heap_budget)
            .map(|(heap, budget)| {
                let device_local = heap.flags.contains(vk::MemoryHeapFlags::DEVICE_LOCAL);
                crate::memory::VulkanHeap::new(heap.size, budget, device_local)
            })
            .collect())
    })?
}

/// What OpenGL gives for `GL_VENDOR` and `GL_VERSION` beneath `adapter`.
#[cfg(not(any(target_vendor = "apple", target_arch = "wasm32")))]
pub(crate) fn gl_strings(adapter: &wgpu::Adapter) -> Result<(String, String), OtherApi> {
    use glow::HasContext as _;
    // SAFETY: the OpenGL adapter is only read from, and is not kept past this function.
    let hal = unsafe { adapter.as_hal::<wgpu::hal::api::Gles>() };
    let opengl = hal.ok_or(OtherApi)?;
    let context = opengl.adapter_context().lock();
    // SAFETY: the call reads a string the context holds, made current on this thread by the lock around it.
    let vendor = unsafe { context.get_parameter_string(glow::VENDOR) };
    // SAFETY: as for the vendor, under the same lock.
    let version = unsafe { context.get_parameter_string(glow::VERSION) };
    drop(context);
    Ok((vendor, version))
}

/// What `read` makes of the Metal device beneath `device`.
#[cfg(target_vendor = "apple")]
pub(crate) fn with_metal<Read, Out>(device: &wgpu::Device, read: Read) -> Result<Out, OtherApi>
where
    Read: FnOnce(&wgpu::hal::metal::Device) -> Out,
{
    // SAFETY: the Metal device is only read from, by `read`, and is not kept past this function.
    let hal = unsafe { device.as_hal::<wgpu::hal::api::Metal>() };
    hal.map(|metal| read(&metal)).ok_or(OtherApi)
}

/// What `read` makes of the architecture the Metal device `raw` gives, `None` for one that gives none.
///
/// objc2-metal types `architecture` as never nil, but an Intel Mac's device answers nil, which its binding turns into
/// a panic, `unexpected NULL returned from -[MTLIGAccelDevice architecture]`. Sending the message directly lets the nil
/// arrive as `None`.
#[cfg(target_vendor = "apple")]
pub(crate) fn with_metal_architecture<Read, Out>(
    raw: &objc2::runtime::ProtocolObject<dyn objc2_metal::MTLDevice>,
    read: Read,
) -> Out
where
    Read: FnOnce(Option<&objc2_metal::MTLArchitecture>) -> Out,
{
    // SAFETY: `architecture` takes no arguments and returns an `MTLArchitecture` or nil, as the type says.
    let architecture: Option<objc2::rc::Retained<objc2_metal::MTLArchitecture>> =
        unsafe { objc2::msg_send![raw, architecture] };
    read(architecture.as_deref())
}

/// The description and user-mode driver version DXGI gives for the adapter beneath `adapter`.
#[cfg(windows)]
pub(crate) fn dxgi_adapter(
    adapter: &wgpu::Adapter,
) -> Result<
    (
        windows::core::Result<windows::Win32::Graphics::Dxgi::DXGI_ADAPTER_DESC2>,
        windows::core::Result<i64>,
    ),
    OtherApi,
> {
    // SAFETY: the DXGI adapter is only read from, and is not kept past this function.
    let hal = unsafe { adapter.as_hal::<wgpu::hal::api::Dx12>() };
    let dx12 = hal.ok_or(OtherApi)?;
    let raw = dx12.raw_adapter();
    // SAFETY: the call only fills in the description it returns, from an adapter that outlives it.
    let description = unsafe { raw.GetDesc2() };
    // SAFETY: the call only fills in the version it returns, from an adapter that outlives it.
    let driver = unsafe { raw.CheckInterfaceSupport(&windows::Win32::Graphics::Dxgi::IDXGIDevice::IID) };
    Ok((description, driver))
}
