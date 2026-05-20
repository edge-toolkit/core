//! Emits the `et:ws-wasi@0.1.0` package — the host-facing API the WASI
//! runner exposes to its guests. Two worlds (`runner`, `module`) and two
//! interfaces (`ws`, `entry`). Everything is statically declared; nothing
//! is derived from `WsMessage`. We use `wit-encoder` so this file mirrors
//! the `et-ws-messages` builder in `src/wit.rs` rather than being a giant
//! raw string.
//!
//! Design notes that previously sat as comments in `wit/world.wit`:
//!
//! * The `ws` interface is **not** a faithful mirror of `WsMessage`. It's a
//!   thin host API: connection lifecycle (`connect` / `disconnect` /
//!   `get-state` / `agent-id`) plus typed `send` and `recv` calls that
//!   carry an `et:ws-messages.ws-message`. Raw out-of-band frames the
//!   server might broadcast (the hub fallback added in ws-broadcast-fix)
//!   surface to the guest as `recv → none`; we don't yet expose them
//!   typed.
//!
//! * `interface entry { run: func() -> result<_, string>; }` is the single
//!   export every WASI module under `et-ws-wasi-runner` must implement.
//!   Returning `err` aborts the runner non-zero.
//!
//! * The `runner` world is what `et-ws-wasi-runner`'s own
//!   `wasmtime::component::bindgen!` consumes. Notably **absent**:
//!     - `wasi:nn/*` and `wasi:io/poll` + `wasi:clocks/*`. The wasi-nn
//!       implementations come from `wasmtime-wasi-nn`'s own
//!       `add_to_linker` (see `src/host/wasi_nn.rs`); clocks + io::poll
//!       come from `wasmtime_wasi::p2::add_to_linker_async`.
//!     - `wasi:webgpu` _is_ included here because the trimmed subset under
//!       `deps/wasi-webgpu/` is wgpu-backed in
//!       `src/host/wasi_webgpu.rs`; replace this whole tree once upstream
//!       wasi-gfx publishes.
//!
//! * The `module` world is what guest WASI modules target. It mirrors
//!   `runner` (`include runner`) and additionally pulls in the
//!   standardised WASI Preview 2 clocks + io::poll (wired by
//!   `wasmtime_wasi::p2::add_to_linker_async`) and the wasi-nn interfaces
//!   (wired through `wasmtime-wasi-nn`). componentize-py generates Python
//!   bindings for every import here.

use wit_encoder::{
    EnumCase, Ident, Include, Interface, Package, PackageName, Params, StandaloneFunc, Type, TypeDef, World,
    WorldNamedInterface,
};

pub fn render() -> String {
    let mut package = Package::new(PackageName::new(
        "et",
        "ws-wasi",
        Some(semver::Version::parse("0.1.0").expect("valid semver")),
    ));
    package.interface(build_ws_interface());
    package.interface(build_entry_interface());
    package.world(build_runner_world());
    package.world(build_module_world());
    package.to_string()
}

/// `interface ws` — host-owned websocket: lifecycle calls plus typed
/// send/recv carrying `et:ws-messages.ws-message`.
fn build_ws_interface() -> Interface {
    let mut iface = Interface::new("ws");
    iface.use_type("et:ws-messages/messages@0.1.0", "ws-message", None);

    iface.type_def(TypeDef::type_("ws-error", Type::String));

    iface.type_def(TypeDef::enum_(
        "state",
        [
            EnumCase::new("connecting"),
            EnumCase::new("connected"),
            EnumCase::new("closing"),
            EnumCase::new("closed"),
        ],
    ));

    let ws_error = Type::Named("ws-error".into());
    let ws_message = Type::Named("ws-message".into());

    iface.function(plain_func("connect", &[], Some(Type::result_err(ws_error.clone()))));
    iface.function(plain_func("get-state", &[], Some(Type::Named("state".into()))));
    iface.function(plain_func("agent-id", &[], Some(Type::String)));
    iface.function(plain_func(
        "send",
        &[("message", ws_message.clone())],
        Some(Type::result_err(ws_error.clone())),
    ));
    iface.function(plain_func(
        "recv",
        &[("timeout-ms", Type::U32)],
        Some(Type::result_both(Type::option(ws_message), ws_error)),
    ));
    iface.function(plain_func("disconnect", &[], None));
    iface
}

/// `interface entry { run: func() -> result<_, string>; }` — the single
/// guest export the runner invokes.
fn build_entry_interface() -> Interface {
    let mut iface = Interface::new("entry");
    iface.function(plain_func("run", &[], Some(Type::result_err(Type::String))));
    iface
}

fn build_runner_world() -> World {
    let mut world = World::new("runner");
    world.named_interface_import(WorldNamedInterface::new("wasi:logging/logging@0.1.0-draft"));
    world.named_interface_import(WorldNamedInterface::new("wasi:keyvalue/store@0.2.0-draft"));
    world.named_interface_import(WorldNamedInterface::new("ws"));
    world.named_interface_import(WorldNamedInterface::new("wasi:webgpu/webgpu@0.0.1"));
    world.named_interface_export(WorldNamedInterface::new("entry"));
    world
}

fn build_module_world() -> World {
    let mut world = World::new("module");
    world.include(Include::new("runner"));
    world.named_interface_import(WorldNamedInterface::new("wasi:clocks/wall-clock@0.2.6"));
    world.named_interface_import(WorldNamedInterface::new("wasi:clocks/monotonic-clock@0.2.6"));
    world.named_interface_import(WorldNamedInterface::new("wasi:io/poll@0.2.6"));
    world.named_interface_import(WorldNamedInterface::new("wasi:nn/tensor@0.2.0-rc-2024-10-28"));
    world.named_interface_import(WorldNamedInterface::new("wasi:nn/graph@0.2.0-rc-2024-10-28"));
    world.named_interface_import(WorldNamedInterface::new("wasi:nn/inference@0.2.0-rc-2024-10-28"));
    world.named_interface_import(WorldNamedInterface::new("wasi:nn/errors@0.2.0-rc-2024-10-28"));
    world
}

fn plain_func(name: &str, params: &[(&str, Type)], result: Option<Type>) -> StandaloneFunc {
    let mut func = StandaloneFunc::new(Ident::new(name.to_string()), /*async_=*/ false);
    let mut p = Params::empty();
    for (pname, pty) in params {
        p.push(Ident::new(pname.to_string()), pty.clone());
    }
    func.set_params(p);
    func.set_result(result);
    func
}
