# Help for `et-ws-server`

`et-ws-server` takes no command-line arguments; it reads its configuration from the environment.

## Environment variables

### `HTTP_BACKLOG`

Type: integer.

Pending connections the listening socket queues before refusing more.

### `HTTP_CLIENT_DISCONNECT_TIMEOUT`

Type: string.

Time a client gets to close a connection before it is dropped, as a humantime duration; `0s` waits forever.

### `HTTP_CLIENT_REQUEST_TIMEOUT`

Type: string.

Time a client gets to send a request's head, as a humantime duration; `0s` waits forever.

### `HTTP_H1_ALLOW_HALF_CLOSED`

Type: boolean.

Whether an HTTP/1 connection stays open after the client half-closes it.

### `HTTP_H1_WRITE_BUFFER_SIZE`

Type: integer.

Size, in bytes, an HTTP/1 response buffer reaches before it is flushed to the connection.

### `HTTP_H2_INITIAL_CONNECTION_WINDOW_SIZE`

Type: integer.

Initial connection-level HTTP/2 flow-control window, in bytes.

### `HTTP_H2_INITIAL_WINDOW_SIZE`

Type: integer.

Initial stream-level HTTP/2 flow-control window, in bytes.

### `HTTP_KEEP_ALIVE`

Type: string.

HTTP keep-alive: `os`, `off`/`disabled`, or a humantime duration such as `5s`.

### `HTTP_MAX_CONNECTIONS`

Type: integer.

Concurrent connections each worker serves.

### `HTTP_MAX_CONNECTION_RATE`

Type: integer.

Concurrent TLS connections each worker handles; its listeners stop accepting new ones at the limit.

### `HTTP_SERVER_HOSTNAME`

Type: string.

Host name the application router uses when it generates URLs.

### `HTTP_SHUTDOWN_TIMEOUT`

Type: integer.

Seconds running connections get to finish once a graceful shutdown starts.

### `HTTP_TCP_NODELAY`

Type: boolean.

Whether `TCP_NODELAY` is set on accepted connections.

### `HTTP_TLS_HANDSHAKE_TIMEOUT`

Type: string.

Time a client gets to complete the TLS handshake, as a humantime duration.

### `HTTP_WORKERS`

Type: integer.

Worker threads per bind address, from 1 to 512; unset, the machine's available parallelism.

### `HTTP_WORKER_MAX_BLOCKING_THREADS`

Type: integer.

Size of each worker's pool of threads for blocking work.

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

### `WS_ADMIN_TOKEN`

Type: string.

Bearer token authorizing the admin API: `PUT /blocked-ips/{ip}` and `DELETE /agents/{agent_id}`.

Unset disables the admin API, which then refuses every request with 403; an empty value is rejected.

### `WS_AGENT_REGISTRY`

Type: string. Default: `registry.yaml`.

The YAML file a local hub loads its agent registry from at startup and saves it to on shutdown.

### `WS_BACKPLANE`

Type: string. Default: `local`.

What the hub shares its agents through: `local` (the default) or a `nats://` / `tls://` URL.

`local` keeps every agent in this process, persisted to the `WS_AGENT_REGISTRY` file. A NATS URL names a server with
`JetStream`, and every hub pointed at it shares one agent directory, one mailbox of unacknowledged direct messages, and
delivery between them, so hubs scale out behind a load balancer: an agent on one hub lists, messages, broadcasts to and
acknowledges agents on any other, and may reconnect to a different hub without losing what was queued for it. The
directory and mailbox live in `JetStream`, so the registry file is neither read nor written in this mode.

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

### `WS_MESSAGE_RETENTION`

Type: string. Default: `latest-per-sender`.

Which unacknowledged direct messages a recipient holds: `latest-per-sender` (the default) or `all`.

`latest-per-sender` keeps only the newest message from each sender, replacing the earlier one, so a recipient holds at
most one message per sender and the hub's memory for them is bounded. `all` keeps every message until it is
acknowledged, with no bound on how many pile up for a recipient that stays away.
