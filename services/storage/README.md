# et-storage-service

Per-agent file storage for the edge-toolkit hub, backed by any [`object_store`](https://docs.rs/object_store)
backend.

The wire protocol (`PUT`/`GET`/`HEAD /storage/{agent_id}/{filename}`) is stable across backends; only the storage
layer beneath it is pluggable. [`StorageConfig::url`] selects the backend and defaults to a `file://` URL under
[`default_storage_folder`], so nothing needs configuring for research use; an `object_store` URL such as
`s3://bucket` points at a remote instead. Objects are addressed as `<agent_id>/<filename>` under whichever store is
in use.
