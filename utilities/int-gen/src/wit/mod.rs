//! Everything that produces or fetches a `.wit` file.
//!
//! * [`messages`] — emits `generated/specs/wit/deps/et-ws-messages/messages.wit`
//!   from the schemars JSON Schema for `WsMessage`.
//! * [`world`] — emits `generated/specs/wit/world.wit` (the host-facing
//!   `et:ws-wasi@0.1.0` package with the `ws`/`entry` interfaces and the
//!   `runner`/`module` worlds).
//! * [`upstream`] — pulls upstream WASI WIT packages into
//!   `generated/specs/wit/deps/<pkg>/` at pinned tags/SHAs, and emits the
//!   hand-trimmed `wasi-webgpu` files (vendored in `src/wit/wasi-webgpu/`).

pub mod messages;
pub mod upstream;
pub mod world;
