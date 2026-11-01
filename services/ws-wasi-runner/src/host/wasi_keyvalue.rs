//! Implements `wasi:keyvalue/store` against the ws-server's storage and modules services via the typed
//! `et-rest-client`, for the buckets [`crate::keyvalue`] names.

use futures_util::StreamExt as _;
use wasmtime::component::Resource;

use crate::HostState;
use crate::bindings::wasi::keyvalue::store::{Error, Host, HostBucket, KeyResponse};
use crate::keyvalue::{Bucket, bucket_from_identifier, kv_not_implemented};

/// Drain a progenitor `ByteStream` into a `Vec<u8>`.
///
/// Used by both bucket kinds since the wasi:keyvalue/store interface returns whole values.
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
        match bucket {
            Bucket::Storage { agent_id } => match self.rest.get_file(agent_id, &key).await {
                Ok(response) => Ok(Some(collect_stream(response.into_inner()).await?)),
                // The OpenAPI spec gives the endpoint a 404 variant, so progenitor surfaces "no such key" as
                // `Error::ErrorResponse`.
                Err(et_rest_client::Error::ErrorResponse(_)) => Ok(None),
                Err(e) => Err(Error::Other(format!("GET {key}: {e}"))),
            },
            // Off the typed call, which would percent-encode the module name as one path segment. A module
            // is served under the name it publishes under, so that name carries the owner scope and holds a
            // `/` and an `@` -- encoded, the request asks for a module no hub serves.
            Bucket::Modules { module_name } => {
                match et_ws_runner_common::fetch_module_file(&self.rest, module_name, &key).await {
                    Ok(bytes) => Ok(Some(bytes)),
                    // Same "no such key" as above, told from the status because a request that did not go
                    // through the generated client has no typed `ErrorResponse` variant to match on.
                    Err(e) if et_ws_runner_common::is_module_file_missing(&e) => Ok(None),
                    Err(e) => Err(Error::Other(format!("GET {key}: {e}"))),
                }
            }
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
