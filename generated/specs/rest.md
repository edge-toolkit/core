# Edge Toolkit REST API

ws-server HTTP surface: health probe, module discovery, module assets, per-agent storage, and
the admin routes that block client addresses and evict agents. The storage routes are an anonymous S3-compatible
interface, addressed path-style as /storage/{agent_id}/{filename} (bucket = agent_id, key = filename), they answer
PUT/GET/HEAD with an ETag, so a standard S3 client can read and write objects without credentials. A client address
the hub has blocked is refused by them with 403. The admin routes take the hub's WS_ADMIN_TOKEN as a bearer token.

API Version: 0.1.0

## Server URLs

- <http://localhost:8080>

## Authentication

- **admin_token**: (http)

## Services

- [admin](#admin)
  - [evict_agent](#admin-delete-agents-agent_id)
  - [block_ip](#admin-put-blocked-ips-ip)
- [health](#health)
  - [health](#health-get-health)
- [modules](#modules)
  - [list_modules_handler](#modules-get-modules)
  - [get_module_file](#modules-get-modules-name-path)
- [storage](#storage)
  - [get_file](#storage-get-storage-agent_id-filename)
  - [head_file](#storage-head-storage-agent_id-filename)
  - [put_file](#storage-put-storage-agent_id-filename)

## admin {#admin}

### evict_agent {#admin-delete-agents-agent_id}

**Operation:** DELETE /agents/{agent_id}
**Description:** The block is the one [`block_ip`] makes, so it takes every agent on that address with it. The direct messages
the agent sent that are still waiting for their recipients are dropped, so nothing it queued is delivered after
it is gone.
**Operation ID:** `evict_agent`

#### Parameters

| Name       | In   | Required | Description    |
| ---------- | ---- | -------: | -------------- |
| `agent_id` | path |      Yes | Agent to evict |

#### Responses

| Code | Type             | Description                           |
| ---- | ---------------- | ------------------------------------- |
| 200  | application/json | Agent evicted and its address blocked |
| 401  |                  | Missing or wrong admin bearer token   |
| 403  |                  | Admin API disabled                    |
| 404  |                  | Agent not found                       |
| 503  |                  | Hub backplane unavailable             |

#### Request Schema

No request schema available.

#### Response Schema

| Field      | Type                                 | Required | Description                                                              |
| ---------- | ------------------------------------ | -------: | ------------------------------------------------------------------------ |
| `response` | [EvictedAgent](#schema-evictedagent) |        - | An agent the hub has evicted, and the address now refused because of it. |

### block_ip {#admin-put-blocked-ips-ip}

**Operation:** PUT /blocked-ips/{ip}
**Description:** Every open socket from that address is closed within a heartbeat, whichever agent it carries, and every later
connection from it is refused. The block outlives a hub restart: a local hub saves it with its registry, and a
NATS-backed hub keeps it in the shared blocklist.

Blocking an address already blocked succeeds again, so the request can be repeated safely. A path segment that
is not an IPv4 or IPv6 address matches no route and is answered 404.
**Operation ID:** `block_ip`

#### Parameters

| Name | In   | Required | Description                   |
| ---- | ---- | -------: | ----------------------------- |
| `ip` | path |      Yes | IPv4 or IPv6 address to block |

#### Responses

| Code | Type | Description                         |
| ---- | ---- | ----------------------------------- |
| 204  |      | Address blocked                     |
| 401  |      | Missing or wrong admin bearer token |
| 403  |      | Admin API disabled                  |
| 404  |      | Not an IP address                   |
| 503  |      | Hub backplane unavailable           |

#### Request Schema

No request schema available.

#### Response Schema

No response schema available.

## health {#health}

### health {#health-get-health}

**Operation:** GET /health
**Description:** Returns a small JSON document identifying the service so external
monitors can confirm the server is reachable and serving requests.
**Operation ID:** `health`

#### Responses

| Code | Type             | Description  |
| ---- | ---------------- | ------------ |
| 200  | application/json | Server is up |

#### Request Schema

No request schema available.

#### Response Schema

| Field      | Type                                     | Required | Description                                                |
| ---------- | ---------------------------------------- | -------: | ---------------------------------------------------------- |
| `response` | [HealthResponse](#schema-healthresponse) |        - | Server liveness probe response. Returned by `GET /health`. |

## modules {#modules}

### list_modules_handler {#modules-get-modules}

**Operation:** GET /modules/
**Description:** List the names of every module the server is currently serving.
**Operation ID:** `list_modules_handler`

#### Responses

| Code | Type             | Description                |
| ---- | ---------------- | -------------------------- |
| 200  | application/json | Names of available modules |

#### Request Schema

No request schema available.

#### Response Schema

| Field        | Type          | Required | Description |
| ------------ | ------------- | -------: | ----------- |
| `response`   | array<string> |        - | -           |
| `response[]` | string        |        - | -           |

### get_module_file {#modules-get-modules-name-path}

**Operation:** GET /modules/{name}/{path}
**Description:** `path` is resolved relative to the module's bundle root; an unknown module or missing file returns 404.

Both path parameters can themselves contain `/`. A module is served under the name its `package.json`
declares, which carries an owner scope (`@scope/name`) for anything published to a registry, and `path`
addresses sub-directories of the bundle. A client that percent-encodes each parameter as one path segment
turns those slashes into `%2F` and asks for something no server serves, so build the request path rather
than passing the values through a per-segment encoder.
**Operation ID:** `get_module_file`

#### Parameters

| Name   | In   | Required | Description                                                   |
| ------ | ---- | -------: | ------------------------------------------------------------- |
| `name` | path |      Yes | Module name, as its package.json declares it -- may be scoped |
| `path` | path |      Yes | Path of the file within the module bundle                     |

#### Responses

| Code | Type                     | Description            |
| ---- | ------------------------ | ---------------------- |
| 200  | application/octet-stream | Static module asset    |
| 404  |                          | No such module or file |

#### Request Schema

No request schema available.

#### Response Schema

No response schema available.

## storage {#storage}

### get_file {#storage-get-storage-agent_id-filename}

**Operation:** GET /storage/{agent_id}/{filename}
**Description:** Download a file previously written to the named agent's storage bucket.
**Operation ID:** `get_file`

#### Parameters

| Name       | In   | Required | Description      |
| ---------- | ---- | -------: | ---------------- |
| `agent_id` | path |      Yes | Agent identifier |
| `filename` | path |      Yes | Stored filename  |

#### Responses

| Code | Type                     | Description            |
| ---- | ------------------------ | ---------------------- |
| 200  | application/octet-stream | Stored file contents   |
| 403  |                          | Client address blocked |
| 404  |                          | No such file           |

#### Request Schema

No request schema available.

#### Response Schema

No response schema available.

### head_file {#storage-head-storage-agent_id-filename}

**Operation:** HEAD /storage/{agent_id}/{filename}
**Description:** Same addressing and 404 handling as [`get_file`], but the response carries headers only: the object's `ETag`
and its size as `Content-Length`. S3 clients issue `HEAD` to stat an object (existence, size, entity tag)
before downloading, so it reports the same `ETag` a `GET` would.
**Operation ID:** `head_file`

#### Parameters

| Name       | In   | Required | Description      |
| ---------- | ---- | -------: | ---------------- |
| `agent_id` | path |      Yes | Agent identifier |
| `filename` | path |      Yes | Stored filename  |

#### Responses

| Code | Type | Description               |
| ---- | ---- | ------------------------- |
| 200  |      | Object metadata (no body) |
| 403  |      | Client address blocked    |
| 404  |      | No such file              |

#### Request Schema

No request schema available.

#### Response Schema

No response schema available.

### put_file {#storage-put-storage-agent_id-filename}

**Operation:** PUT /storage/{agent_id}/{filename}
**Description:** Only the agent that owns the bucket may write to it: the agent must be connected, and the request must come from
the address it is connected from. The path component must be a single filename, not a nested path.
**Operation ID:** `put_file`

#### Parameters

| Name          | In   | Required | Description                                  |
| ------------- | ---- | -------: | -------------------------------------------- |
| `agent_id`    | path |      Yes | Agent identifier (must be a connected agent) |
| `filename`    | path |      Yes | Single-segment filename to write             |
| `requestBody` | body |      Yes | Raw file bytes                               |

#### Responses

| Code | Type | Description                                                           |
| ---- | ---- | --------------------------------------------------------------------- |
| 200  |      | File stored                                                           |
| 400  |      | Invalid filename                                                      |
| 403  |      | Address blocked, agent not connected, or request not from its address |
| 404  |      | Agent not found                                                       |

#### Request Schema

| Field     | Type           | Required | Description                                                                                                                                                                                                             |
| --------- | -------------- | -------: | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `request` | string(binary) |        - | Phantom type used to label binary request/response bodies as `string`/`binary`. Never constructed at runtime; only exists under the `openapi-spec` feature so the `utoipa::ToSchema` derive has something to attach to. |

#### Response Schema

No response schema available.

## Schema Definitions

### EvictedAgent {#schema-evictedagent}

| Field                     | Type   | Required | Description                                                              |
| ------------------------- | ------ | -------: | ------------------------------------------------------------------------ |
| `EvictedAgent`            | object |        - | An agent the hub has evicted, and the address now refused because of it. |
| `EvictedAgent.agent_id`   | string |      Yes | -                                                                        |
| `EvictedAgent.blocked_ip` | string |      Yes | -                                                                        |

### HealthResponse {#schema-healthresponse}

| Field                    | Type   | Required | Description                                                |
| ------------------------ | ------ | -------: | ---------------------------------------------------------- |
| `HealthResponse`         | object |        - | Server liveness probe response. Returned by `GET /health`. |
| `HealthResponse.service` | string |      Yes | -                                                          |
| `HealthResponse.status`  | string |      Yes | -                                                          |
