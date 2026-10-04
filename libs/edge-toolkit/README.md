# edge-toolkit

Edge Toolkit is a framework for running workloads across a fleet of devices -- browser tabs, phones, native
processes -- that all connect to one WebSocket hub. Agents register with the hub, exchange messages through it, and
keep per-agent files in its storage. A workload is a ws-module -- written in Rust, Python, JavaScript, Dart, Kotlin,
C#, Java, R or Zig -- and runs either in a browser or under one of the native runners.

This crate is the code every part of that system shares:

- `ws` -- the wire protocol: the `ClientMessage` agents send and the `ServerMessage` the hub replies with, plus the
  agent summaries and delivery states they carry.
- `ws_server` -- the agent registry the hub keeps, with its persisted records and queued direct messages.
- `config` -- shared configuration helpers, including the project-root lookup and the default module folders.
- `ports` -- the well-known port of each service.
- `auth` and `args` -- small helpers for basic-auth credentials and the running executable's name.

## The hub and the runners

- [`et-ws-server`](https://crates.io/crates/et-ws-server) -- the hub: agent registry, message routing, storage and
  modules.
- [`et-ws-wasi-runner`](https://crates.io/crates/et-ws-wasi-runner) -- runs ws-modules compiled to WASI Preview 2
  components.
- [`et-ws-web-runner`](https://crates.io/crates/et-ws-web-runner) -- runs browser-targeted ws-modules natively under
  embedded Deno.
- [`et-ws-pyo3-runner`](https://crates.io/crates/et-ws-pyo3-runner) -- runs an agent written in plain Python through
  embedded CPython.

The source, the AsyncAPI and OpenAPI specs, and every ws-module live in the
[edge-toolkit repository](https://github.com/edge-toolkit/core).
