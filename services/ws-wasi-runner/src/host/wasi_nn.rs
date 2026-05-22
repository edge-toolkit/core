//! Glue between `wasmtime-wasi-nn` and our `HostState`.
//!
//! `wasmtime-wasi-nn` ships its own `bindgen!`-generated implementation of the
//! `wasi:nn` WIT interfaces; we just need to construct a `WasiNnCtx` with the
//! ONNX backend, expose a `WasiNnView` from our `HostState`, and call its
//! `add_to_linker` so the linker accepts components that import `wasi:nn/*`.
//!
//! Backend choice: the runner builds with `wasmtime-wasi-nn`'s `onnx` feature,
//! which routes inference through `ort` (ONNX Runtime). On macOS that means
//! the system `CoreML` / Apple Accelerate paths; on Linux it picks up CUDA /
//! `ROCm` if the system ONNX Runtime was built with them, else CPU.
//!
//! Why no `wasi:webgpu` integration here: wasi-nn delegates to a host-chosen
//! ML runtime which manages its own GPU access. The trimmed `wasi:webgpu`
//! interface remains the path for "raw" GPU compute from guests; `wasi-nn`
//! is the path for "popular ML pattern" (load model, set input, compute,
//! get output).

use wasmtime_wasi_nn::wit::{WasiNnCtx, WasiNnView};

/// Build a `WasiNnCtx` configured with whatever backends the crate's feature flags enabled (just `onnx` for us).
///
/// Empty registry — guests load model bytes directly via `graph.load`, so
/// name-based lookup isn't needed.
#[must_use]
pub fn new_ctx() -> WasiNnCtx {
    let backends = wasmtime_wasi_nn::backend::list();
    let registry = wasmtime_wasi_nn::Registry::from(wasmtime_wasi_nn::InMemoryRegistry::new());
    WasiNnCtx::new(backends, registry)
}

pub fn view(state: &mut crate::HostState) -> WasiNnView<'_> {
    WasiNnView::new(&mut state.resource_table, &mut state.wasi_nn_ctx)
}
