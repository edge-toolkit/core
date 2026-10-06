//! The binding tree `wit_bindgen::generate!` emits for the `module` world, re-exported at the crate root.
#![expect(
    clippy::collection_is_never_read,
    clippy::error_impl_error,
    clippy::exhaustive_enums,
    clippy::exhaustive_structs,
    clippy::mem_forget,
    clippy::same_length_and_capacity,
    reason = "wit-bindgen's generated code, which this crate does not control; the module holds nothing else"
)]

wit_bindgen::generate!({
    // ET_WIT_DIR is the absolute path to generated/specs/wit, emitted by build.rs.
    path: env!("ET_WIT_DIR"),
    world: "module",
    generate_all,
    // Let a guest crate invoke the generated `export!` and have the expansion resolve its type paths back here
    // rather than in the guest, which is what makes one binding tree serve every guest.
    pub_export_macro: true,
    default_bindings_module: "et_wasi_guest::bindings",
});
