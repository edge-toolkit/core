//! Implements `wasi:keyvalue/store` against the ws-server's storage and
//! modules services. The bucket identifier names a path-prefix:
//!
//! * `<agent-uuid>` → bucket prefix `/storage/{agent-uuid}/`. Reads work for
//!   any agent's bucket (the server static-serves everything under
//!   `/storage/`); writes only succeed when the runner's own agent owns the
//!   bucket (server enforces `agent_id` is registered).
//! * `modules/<module-name>` → bucket prefix `/modules/<module-name>/`. Used
//!   by guests to fetch their own static assets bundled in `pkg/`. Writes
//!   return `access-denied` since et-modules-service serves files static.
//!
//! The `Bucket` resource is just a thin owner of the prefix string; the
//! HTTP work happens in `get` / `set`.

use wasmtime::component::Resource;

use crate::HostState;
use crate::bindings::wasi::keyvalue::store::{Error, Host, HostBucket, KeyResponse};
use crate::host::KvErrExt;

pub struct Bucket {
    /// URL path-prefix on the ws-server, including the leading slash and
    /// trailing slash. Keys are appended verbatim.
    prefix: String,
    /// Whether this bucket accepts writes. False for `modules/...` buckets.
    writable: bool,
}

impl Bucket {
    fn url(&self, http_base: &str, key: &str) -> String {
        format!("{http_base}{}{key}", self.prefix)
    }
}

/// Map a `store.open` identifier to a bucket prefix and writability.
fn bucket_from_identifier(identifier: &str) -> Result<Bucket, Error> {
    if let Some(module_name) = identifier.strip_prefix("modules/") {
        if module_name.is_empty() || module_name.contains('/') {
            return Err(Error::Other(format!(
                "invalid module bucket identifier: {identifier:?}"
            )));
        }
        return Ok(Bucket {
            prefix: format!("/modules/{module_name}/"),
            writable: false,
        });
    }
    if identifier.is_empty() || identifier.contains('/') {
        return Err(Error::Other(format!("invalid bucket identifier: {identifier:?}")));
    }
    Ok(Bucket {
        prefix: format!("/storage/{identifier}/"),
        writable: true,
    })
}

impl Host for HostState {
    async fn open(&mut self, identifier: String) -> Result<Resource<Bucket>, Error> {
        let bucket = bucket_from_identifier(&identifier)?;
        let res = self.resource_table.push(bucket).kv_context("resource table push")?;
        Ok(res)
    }
}

impl HostBucket for HostState {
    async fn get(&mut self, rep: Resource<Bucket>, key: String) -> Result<Option<Vec<u8>>, Error> {
        let bucket = self.resource_table.get(&rep).kv_context("bucket handle")?;
        let url = bucket.url(&self.http_base, &key);
        let resp = self.http.get(&url).send().await.kv_context(&format!("GET {url}"))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(Error::Other(format!("GET {url}: HTTP {}", resp.status())));
        }
        let bytes = resp.bytes().await.kv_context(&format!("GET {url} body"))?;
        Ok(Some(bytes.to_vec()))
    }

    async fn set(&mut self, rep: Resource<Bucket>, key: String, value: Vec<u8>) -> Result<(), Error> {
        let bucket = self.resource_table.get(&rep).kv_context("bucket handle")?;
        if !bucket.writable {
            return Err(Error::AccessDenied);
        }
        let url = bucket.url(&self.http_base, &key);
        let resp = self
            .http
            .put(&url)
            .body(value)
            .send()
            .await
            .kv_context(&format!("PUT {url}"))?;
        if !resp.status().is_success() {
            return Err(Error::Other(format!("PUT {url}: HTTP {}", resp.status())));
        }
        Ok(())
    }

    async fn delete(&mut self, _rep: Resource<Bucket>, _key: String) -> Result<(), Error> {
        Err(Error::Other("delete not implemented".into()))
    }

    async fn exists(&mut self, _rep: Resource<Bucket>, _key: String) -> Result<bool, Error> {
        Err(Error::Other("exists not implemented".into()))
    }

    async fn list_keys(&mut self, _rep: Resource<Bucket>, _cursor: Option<String>) -> Result<KeyResponse, Error> {
        Err(Error::Other("list-keys not implemented".into()))
    }

    async fn drop(&mut self, rep: Resource<Bucket>) -> wasmtime::Result<()> {
        self.resource_table.delete(rep)?;
        Ok(())
    }
}
