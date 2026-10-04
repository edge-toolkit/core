# Help for `et-ws-web-runner`

`et-ws-web-runner` takes no command-line arguments; it reads its configuration from the environment.

## Environment variables

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

### `RUNNER_MODULE`

Type: string. Required.

Module to run, from `RUNNER_MODULE` (required).

### `RUNNER_TIMEOUT`

Type: string.

Optional wall-clock timeout, from `RUNNER_TIMEOUT` (e.g. `120s`, `3m`); `None` runs without a timeout.

### `V8_FLAGS`

Type: string.

Optional V8 flags from `V8_FLAGS`, applied via `v8::V8::set_flags_from_string` before the runtime initialises.

Used to select the WASM compile tier when debugging the gnullvm dotnet-data1 crash (e.g. `--no-liftoff`,
`--liftoff-only`, `--jitless`).

### `WS_CONNECT_ACK_TIMEOUT`

Type: string. Default: `5s`.

How long `crate::connect_and_register` waits for the server's `et-connect-ack`.

Read from `WS_CONNECT_ACK_TIMEOUT` as a humantime duration (e.g. `5s`, `500ms`). Unset defaults to 5s;
`none`/`off`/`disabled` waits forever (retry until the server answers).

### `WS_SERVER_URL`

Type: string. Default: `ws://localhost:8080/ws`.

ws-server URL, from `WS_SERVER_URL`; defaults to the local insecure port.
