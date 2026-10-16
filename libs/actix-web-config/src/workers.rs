//! The worker count, validated to the range actix-server accepts.
#![expect(
    clippy::exhaustive_enums,
    reason = "nutype generates `WorkersError` as an exhaustive enum of the two bounds it checks"
)]

use nutype::nutype;

/// Number of worker threads, which actix-server requires to be from 1 to 512 and panics outside.
#[nutype(
    validate(greater_or_equal = 1, less_or_equal = 512),
    derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)
)]
pub struct Workers(usize);
