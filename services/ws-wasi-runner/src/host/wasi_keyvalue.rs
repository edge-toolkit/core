//! Implements `wasi:keyvalue/store` against the ws-server's storage and
//! modules services via the typed `et-rest-client`. The bucket identifier
//! names a namespace:
//!
//! * `<agent-uuid>` -> per-agent storage bucket. Reads work for any agent's
//!   bucket (server static-serves everything under `/storage/`); writes only
//!   succeed when the runner's own agent owns the bucket (server enforces
//!   `agent_id` is registered).
//! * `modules/<module-name>` -> module asset bucket. Used by guests to fetch
//!   their own static assets bundled in `pkg/`. Writes return
//!   `access-denied` since et-modules-service serves files static.

use futures_util::StreamExt as _;
use wasmtime::component::Resource;

use crate::HostState;
use crate::bindings::wasi::keyvalue::store::{Error, Host, HostBucket, KeyResponse};
use crate::host::kv_not_implemented;

/// Bucket-kind discriminator. The wire prefix on the ws-server is implied by
/// the variant; the typed REST client picks the right operation.
#[non_exhaustive]
pub enum Bucket {
    /// `/storage/{agent_id}/` -- writable, owned by the named agent.
    Storage { agent_id: String },
    /// `/modules/{module_name}/` -- read-only static module assets.
    Modules { module_name: String },
}

/// Map a `store.open` identifier to a bucket variant.
#[expect(
    clippy::single_call_fn,
    reason = "named identifier parser; used once by <HostState as Host>::open"
)]
fn bucket_from_identifier(identifier: &str) -> Result<Bucket, Error> {
    if let Some(module_name) = identifier.strip_prefix("modules/") {
        if module_name.is_empty() || module_name.contains('/') {
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

/// Drain a progenitor `ByteStream` into a `Vec<u8>`. Used by both bucket
/// kinds since the wasi:keyvalue/store interface returns whole values.
#[expect(
    clippy::single_call_fn,
    reason = "named helper; used once by <HostState as HostBucket>::get"
)]
async fn collect_stream(mut stream: et_rest_client::ByteStream) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

#[expect(
    clippy::unused_async_trait_impl,
    reason = "wasi:keyvalue/store's generated Host trait declares every method async; the impl cannot drop it"
)]
impl Host for HostState {
    async fn open(&mut self, identifier: String) -> Result<Resource<Bucket>, Error> {
        let bucket = bucket_from_identifier(&identifier)?;
        let res = self.resource_table.push(bucket)?;
        Ok(res)
    }
}

#[expect(
    clippy::unused_async_trait_impl,
    reason = "generated HostBucket trait declares every method async; the not-implemented ones never await"
)]
impl HostBucket for HostState {
    async fn get(&mut self, self_: Resource<Bucket>, key: String) -> Result<Option<Vec<u8>>, Error> {
        let bucket = self.resource_table.get(&self_)?;
        let result = match bucket {
            Bucket::Storage { agent_id } => self.rest.get_file(agent_id, &key).await,
            Bucket::Modules { module_name } => self.rest.get_module_file(module_name, &key).await,
        };
        match result {
            Ok(response) => Ok(Some(collect_stream(response.into_inner()).await?)),
            // The OpenAPI spec gives both endpoints a 404 variant, so progenitor
            // surfaces "no such key" as `Error::ErrorResponse`.
            Err(et_rest_client::Error::ErrorResponse(_)) => Ok(None),
            Err(e) => Err(Error::Other(format!("GET {key}: {e}"))),
        }
    }

    async fn set(&mut self, self_: Resource<Bucket>, key: String, value: Vec<u8>) -> Result<(), Error> {
        let bucket = self.resource_table.get(&self_)?;
        let agent_id = match bucket {
            Bucket::Storage { agent_id } => agent_id.clone(),
            Bucket::Modules { .. } => return Err(Error::AccessDenied),
        };
        let _response = self.rest.put_file(&agent_id, &key, value).await?;
        Ok(())
    }

    async fn delete(&mut self, _self_: Resource<Bucket>, _key: String) -> Result<(), Error> {
        Err(kv_not_implemented("delete"))
    }

    async fn exists(&mut self, _self_: Resource<Bucket>, _key: String) -> Result<bool, Error> {
        Err(kv_not_implemented("exists"))
    }

    async fn list_keys(&mut self, _self_: Resource<Bucket>, _cursor: Option<String>) -> Result<KeyResponse, Error> {
        Err(kv_not_implemented("list-keys"))
    }

    async fn drop(&mut self, rep: Resource<Bucket>) -> wasmtime::Result<()> {
        let _removed: Bucket = self.resource_table.delete(rep)?;
        Ok(())
    }
}
