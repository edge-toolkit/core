//! `wasi:keyvalue/store` buckets as every host names them: what a `store.open` identifier opens, and the errors a
//! bucket call can fail with.
//!
//! The bucket identifier names a namespace:
//!
//! * `<agent-uuid>` -> per-agent storage bucket. Reads work for any agent's bucket (server static-serves everything
//!   under `/storage/`); writes only succeed when the runner's own agent owns the bucket (server enforces `agent_id`
//!   is registered).
//! * `modules/<module-name>` -> module asset bucket. Used by guests to fetch their own static assets bundled in
//!   `pkg/`. Writes return `access-denied` since et-modules-service serves files static.

#[cfg(not(feature = "std"))]
use alloc::{
    format,
    string::{String, ToString as _},
};

use crate::bindings::wasi::keyvalue::store::Error;

/// Bucket-kind discriminator.
///
/// The wire prefix on the ws-server is implied by the variant; the typed REST client picks the right operation.
#[non_exhaustive]
pub enum Bucket {
    /// `/storage/{agent_id}/` -- writable, owned by the named agent.
    Storage { agent_id: String },
    /// `/modules/{module_name}/` -- read-only static module assets.
    Modules { module_name: String },
}

/// True when `candidate` names one module, rather than addressing a path through one.
///
/// A module is served under the name it publishes under, so a scoped name carries exactly one `/`, directly
/// after a leading `@`. Any other separator is a path, which a bucket identifier may not express -- the key
/// passed to `get` is what addresses a file inside the bucket.
#[expect(
    clippy::single_call_fn,
    reason = "named predicate; used once by bucket_from_identifier and kept separate so the shape is readable"
)]
fn names_one_module(candidate: &str) -> bool {
    let Some(scoped) = candidate.strip_prefix('@') else {
        return !candidate.is_empty() && !candidate.contains('/');
    };
    match scoped.split_once('/') {
        Some((scope, name)) => !scope.is_empty() && !name.is_empty() && !name.contains('/'),
        None => false,
    }
}

/// Map a `store.open` identifier to a bucket variant.
///
/// # Errors
///
/// When `identifier` is empty, or is a path rather than one agent's id or one module's name.
pub fn bucket_from_identifier(identifier: &str) -> Result<Bucket, Error> {
    if let Some(module_name) = identifier.strip_prefix("modules/") {
        if !names_one_module(module_name) {
            return Err(Error::Other(format!(
                "invalid module bucket identifier: {identifier:?}"
            )));
        }
        return Ok(Bucket::Modules {
            module_name: module_name.to_string(),
        });
    }
    if identifier.is_empty() || identifier.contains('/') {
        return Err(Error::Other(format!("invalid bucket identifier: {identifier:?}")));
    }
    Ok(Bucket::Storage {
        agent_id: identifier.to_string(),
    })
}

/// Build a `wasi:keyvalue/store.error.other("<op> not implemented")`.
/// That is the closest thing the WIT-spec enum has to a `NotImplemented` variant.
#[must_use]
pub fn kv_not_implemented(operation: &str) -> Error {
    Error::Other(format!("{operation} not implemented"))
}

/// A bucket handle the guest passed that its store no longer holds.
impl From<wasmtime::component::ResourceTableError> for Error {
    fn from(err: wasmtime::component::ResourceTableError) -> Self {
        Self::Other(err.to_string())
    }
}
