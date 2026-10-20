//! The worker count, defaulting to the machine's parallelism and validated to the range actix-server accepts.
#![expect(
    clippy::exhaustive_enums,
    reason = "nutype generates `WorkersError` as an exhaustive enum of the two bounds it checks"
)]

use std::num::NonZeroUsize;
use std::thread::available_parallelism;

use nutype::nutype;

/// The most workers actix-server accepts: `HttpServer::workers` panics on anything above it.
pub const MAX_WORKERS: usize = 512;

/// Number of worker threads, from 1 to [`MAX_WORKERS`]; by default, the machine's available parallelism.
#[nutype(
    validate(greater_or_equal = 1, less_or_equal = MAX_WORKERS),
    default = default_workers(),
    derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)
)]
pub struct Workers(usize);

/// [`available_parallelism`], held to [`MAX_WORKERS`] so a machine with more cores than that still starts.
///
/// Where the parallelism cannot be determined this is 2, the fallback actix-server itself uses in that case.
#[expect(
    clippy::single_call_fn,
    reason = "nutype's `default =` takes one expression; naming it keeps the fallback documented"
)]
fn default_workers() -> usize {
    available_parallelism().map_or(2, NonZeroUsize::get).min(MAX_WORKERS)
}
