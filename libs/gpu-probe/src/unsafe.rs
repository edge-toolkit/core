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

/// The experimental features a device is opened with when the matrix probes are to run: cooperative matrices.
pub(crate) const fn experimental_features() -> wgpu::ExperimentalFeatures {
    // SAFETY: the one experimental feature asked for is cooperative matrices, which only the matrix probes use; each
    // runs in a shader of its own, and a probe the device mishandles answers with its refusal.
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
/// objc2-metal types `architecture` as never nil, but an Intel Mac's device answers nil, which its binding turns into a
/// panic, `unexpected NULL returned from -[MTLIGAccelDevice architecture]`. Sending the message directly lets the nil
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
