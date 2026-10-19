# et-ws-server

The edge-toolkit hub server: the `et-ws-service` agent hub at `/ws`, which browser tabs, native runners and scripts
connect to, packaged as a binary with configuration, logging and a startup banner.

Around that hub it can mount per-agent file storage, a static server for ws-module packages and the browser UI, and
a `/websockify` relay for in-browser runtimes. Each is a cargo feature, all on by default.

## Running

```sh
cargo install et-ws-server
et-ws-server
```

It listens for HTTP on port 8080 and, with the `tls` feature, for HTTPS/WSS on 8443, logging every reachable
address and a QR code of the one phones on the same network should open.

It takes no command-line arguments: all configuration is read from the environment.
[HELP.md](https://github.com/edge-toolkit/core/blob/main/services/ws-server/HELP.md) lists every environment
variable, with its type and default.

## Choosing services

A hub that only routes messages needs none of the optional services:

```toml
[dependencies]
et-ws-server = { version = "0.2", default-features = false }
```

Add back what the deployment serves, e.g. `features = ["storage", "tls"]` for a headless hub whose agents exchange
files over HTTPS. The wire protocol and REST surface are published as AsyncAPI and OpenAPI specs in the
[edge-toolkit repository](https://github.com/edge-toolkit/core/tree/main/generated/specs).
