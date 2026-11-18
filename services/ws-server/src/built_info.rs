//! What `built` recorded about this binary's build, written by the build script.
#![expect(
    clippy::allow_attributes,
    clippy::needless_raw_strings,
    reason = "the generated file carries its own allow attributes and quotes every value as a raw string"
)]

include!(concat!(env!("OUT_DIR"), "/built.rs"));
