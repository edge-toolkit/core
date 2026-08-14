//! `wasmtime::component::bindgen!` output for the runner world.
//!
//! Lives in its own file so `no-inline-mod` can stay enforced on the
//! crate root: the macro generates a `mod`-shaped tree of types, which
//! would otherwise have to be wrapped in `pub mod bindings { ... }` at
//! the `lib.rs` top level.
#![expect(
    clippy::error_impl_error,
    clippy::exhaustive_enums,
    clippy::exhaustive_structs,
    clippy::impl_trait_in_params,
    clippy::integer_division_remainder_used,
    clippy::missing_asserts_for_indexing,
    reason = "wasmtime::component::bindgen! generates the API surface from WIT; we don't control its lints"
)]

//! wasmtime-bindgen output for the `runner` world declared in
//! `generated/specs/wit/world.wit`. Every WIT type defined in the world or
//! its dep packages is reachable through `crate::bindings::<namespace>`.
//!
//! The `runner` world deliberately omits wasi-nn and wasi-webgpu: their host
//! sides are external crates that register themselves on the linker, so
//! `world module` (what the guest targets) is where those imports are
//! declared.
wasmtime::component::bindgen!({
    // Sanctioned `..` exception: unlike `wit_bindgen::generate!` (the guest
    // modules), wasmtime's `bindgen!` has no macro-string support, so `path:`
    // can't be an `env!(...)` fed by build.rs -- it must be a literal resolved
    // against this crate's dir. Kept relative-to-manifest as the one place a
    // repo-relative `..` is unavoidable.
    path: "../../generated/specs/wit",
    world: "runner",
    imports: { default: async },
    exports: { default: async },
    with: {
        "wasi:keyvalue/store.bucket": super::host::wasi_keyvalue::Bucket,
    },
});
