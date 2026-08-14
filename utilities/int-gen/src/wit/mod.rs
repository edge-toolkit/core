//! Everything that produces or fetches a `.wit` file.
//!
//! * [`messages`] -- emits `generated/specs/wit/deps/et-ws-messages/messages.wit`
//!   from the schemars JSON Schemas for `ClientMessage` and `ServerMessage`.
//! * [`upstream`] -- pulls upstream WASI WIT packages into
//!   `generated/specs/wit/deps/<pkg>/` at pinned tags/SHAs.
//!
//! The top-level `et:ws-wasi@0.1.0` package (`generated/specs/wit/world.wit`)
//! is **not** generated -- it's hand-maintained. See `generated/README.md`.

pub mod messages;
pub mod upstream;
