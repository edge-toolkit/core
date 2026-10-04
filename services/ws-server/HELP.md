# Command-Line Help for `et-ws-server`

This document contains the help content for the `et-ws-server` command-line program.

**Command Overview:**

- [`et-ws-server`↴](#et-ws-server)

## `et-ws-server`

Actix-web entry point wiring the ws hub, storage and modules services together

**Usage:** `et-ws-server [OPTIONS]`

### Options

- `-a`, `--agent-registry <AGENT_REGISTRY>` — Path to agent registry YAML file

  Default value: `registry.yaml`

## Environment variables

### `MODULES_MISE_DISCOVER`

Type: boolean.

Also serve whatever the mise config in scope staged, on top of `paths`.

A deployment that installs its modules as `[tools]` has already said which ones it serves. Repeating that as a list of
directories would be the same set written twice, in a form nothing can write down -- where mise puts a package is
decided per backend and platform when it installs.

Defaults to whether mise is there to ask, because mise being on `PATH` is exactly what makes a staged module findable: a
deployment that provisioned its modules some other way has no mise to consult and gets nothing extra, and one that did
needs to declare nothing. Set it to `false` to serve only `paths` on a host that does have mise -- a config whose tool
set mixes modules with development tooling wants that, since discovery cannot tell one from the other.

### `MODULES_PATHS`

Type: comma-separated list of string.

Directories scanned for ws-module packages, comma-separated in the environment.

Defaults to the workspace's standard module folders, resolved against the working directory; a folder that does not
exist is skipped.

### `MODULES_ROOT`

Type: string. Default: empty.

Name of the module served at `/`, exactly as its `package.json` declares it.

No default, because which module is a deployment's front page is a property of that deployment and not of this server: a
name defaulted here would be one project's, and every other one would be carrying it around as dead configuration. Unset
serves nothing at `/` and is not an error -- a deployment whose agents are headless runners has no page to put there,
and demanding one would make every such deployment name a module it never loads.

### `NET_LOG_INTERFACE`

Type: string.

Name of the interface whose address the startup banner and QR code should advertise first.

Set it to `bridge100` when the machine shares its internet connection as a Wi-Fi hotspot: macOS Internet Sharing puts
the hotspot's gateway address there, and it is the only address the devices scanning the QR code can reach. Naming an
interface makes it a hard requirement -- startup fails if it is absent or holds no usable IPv4 address. When unset,
ranking is automatic: `en*` NICs first, then `bridge*`, then everything else.

### `OTLP_AUTH_PASSWORD`

Type: string.

Password.

### `OTLP_AUTH_USERNAME`

Type: string.

Username.

### `OTLP_COLLECTOR_URL`

Type: string. Default: `http://127.0.0.1:5080/api/default/v1`.

OpenTelemetry collector URL.

### `OTLP_PROTOCOL`

Type: one of `Binary`, `JSON`. Default: `Binary`.

OpenTelemetry protocol.

### `OTLP_SERVICE_LABEL`

Type: string.

OpenTelemetry service label.

Defaults to the running executable's name, without any `-server` in it.

### `STORAGE_URL`

Type: string.

`object_store` backend URL, defaulting to local disk under `default_storage_folder`.

One field rather than a path plus an optional override: the backend has exactly one source of truth, and the default is
declared here rather than reconstructed inside `build_store`. Anything `object_store` recognises for a compiled-in
backend works -- a `file:` URL naming an absolute directory, `s3://bucket` (including S3-compatible servers),
`memory://`. Credentials, endpoint and addressing style come from each backend's own standard environment variables
rather than config keys of our own, so an operator configures a store exactly as they would for any other client of it.

### `TLS_CERT_FILE`

Type: string. Default: `cert.pem`.

### `TLS_KEY_FILE`

Type: string. Default: `key.pem`.

### `WS_CONNECTION_TIMEOUT`

Type: string. Default: `15s`.

Idle period before the hub closes a connection, as a humantime duration (e.g. `15s`, `1m30s`).

Unset defaults to 15s; `none`/`off`/`disabled` turns the idle timeout off (the hub never closes a connection for
inactivity), which suits a frontend that sits idle.

### `WS_MAX_FRAME_SIZE`

Type: string. Default: `64MiB`.

Largest single WebSocket frame the hub will accept.

Frames above this are dropped by actix-ws before they reach the handler, so callers shipping big tensors / blobs need to
raise it above their payload size. `WS_MAX_FRAME_SIZE` takes a human byte size (e.g. `64MiB`, `64MB`, `512KiB`) or a
plain byte count; unset defaults to 64 MiB.
