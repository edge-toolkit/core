# Changelog

All notable changes to this project are recorded here, one section per version tag. The format follows Keep a
Changelog 1.1.0.

## [Unreleased]

### Added

- A scenario can list `machines:` to span several hosts. The first is the server, running the hub and the collector,
  and each agent runs on the machine that lists it. k3s pins each `Deployment` with a node selector on
  `edge-toolkit/machine`, compose gives each machine a profile, mise adds a `machine-<name>` task per machine, and
  dekit tags each task with its machine. A runner off the server reaches the hub at the server's `address`. A
  scenario without `machines:` renders as before.
- A scenario can set resource limits per process: `limits:` on an agent, and `limits:` on `services.hub` and
  `services.collector`, a new top-level table of settings for the hub and the collector. The fields are every
  resource control compose enforces on cgroup v2 without naming a host device. Compose and k3s both apply each one
  to the same cgroup value. k3s does so for the fields the Pod API lacks through the upstream `ulimit-adjuster` and
  `nri-memory-qos` NRI plugins, refusing a limited pod on a node without them. Every compose service now runs
  under the CPU and memory defaults the k3s manifests have always set.
- A new output type, `process-compose`, writes a `process-compose.yaml` that runs the `mise.toml` tasks under
  process-compose, each waiting on what it talks to being ready, as `dekit.yaml` does.
- An agent with a runner can set `schedule:` -- a five-field `cron` expression and an optional IANA `time_zone` -- to
  run as a job, started on each tick and skipped while the previous run is still going. k3s renders it as a
  `CronJob`, compose adds an Ofelia `scheduler` service that restarts the runner's container on each tick, and
  process-compose schedules the task itself. Generating mise or dekit for a scheduled scenario is an error.
- `et-actix-web-config` 0.1.0: actix-web's `HttpServer` settings as a serde struct, for reading them from environment
  variables. Each unset setting keeps actix's own default, and a value actix would panic on is rejected while it is
  read. It depends on no other crate of this project, so it can be used on its own.
- `et-ws-server` reads its `HttpServer` settings from `HTTP_*` variables, such as `HTTP_WORKERS`,
  `HTTP_KEEP_ALIVE` and `HTTP_SHUTDOWN_TIMEOUT`.
- Hubs scale out. `WS_BACKPLANE=nats://<host>:<port>` points several `et-ws-server` instances at one NATS server
  with JetStream, and they then share one agent directory, one mailbox of unacknowledged direct messages and
  delivery between them. An agent on any hub lists, messages, broadcasts to and acknowledges agents on every other,
  uploads to storage through any hub, and can reconnect to a different hub without losing what was queued for it.
  The default, `local`, behaves as before. In NATS mode the `WS_AGENT_REGISTRY` file is neither read nor written. The
  new `et-hub-backplane` crate holds the NATS side, and `et-test-nats` starts a throwaway broker for tests.

### Changed

- The workspace requires actix-web 4.17.
- **Breaking:** an agent's IP address is always known. `AgentSummary::last_known_ip` and
  `AgentRecord::last_known_ip` are `IpAddr` rather than `Option<String>`, and `AgentRegistry::connect_agent` takes
  an `IpAddr`. On the wire, `last_known_ip` in an `et-list-agents-response` is a required IP string, never null,
  and the generated Dart and WIT bindings follow suit. A `registry.yaml` holding a null or non-IP
  `last_known_ip` no longer loads.
- **Breaking:** the hub takes a client's IP from the socket peer only, ignoring `Forwarded` and `X-Forwarded-For`,
  so behind a reverse proxy every agent reports the proxy's address. A request with no peer address, such as one
  over a Unix domain socket, is refused, and `et-ws-server` listens on TCP only.
- **Breaking:** `et-ws-server` takes no command-line arguments. Its one flag, `-a` / `--agent-registry`, is now the
  `WS_AGENT_REGISTRY` environment variable, still defaulting to `registry.yaml`, and `et_ws_server::cli` is gone.
  Generated k3s manifests set the variable instead of overriding the container's `command` and `args`.

## [0.3.0] - 2026-10-15

### Changed

- `et-client-event` carries a CloudEvents 1.0 envelope under `event`, with `type` `et.<capability>.<action>`,
  `source` the producing module's served path, and `data` the former `details`. The flat `capability`, `action` and
  `details` fields are gone, so senders and hubs from before this change cannot exchange client events (#140,
  #141).
- The envelope's `time` is a typed timestamp: `CloudEvent::new` takes a `DateTime<Utc>`, and the schema declares
  `format: date-time`.
- The envelope's `specversion` is the `SpecVersion` enum. WIT carries it as a `spec-version` string alias, Dart as
  `SpecVersion.v1_0`, Python as `SpecVersion.field_1_0`.
- An event whose `time` is not an RFC 3339 timestamp, or whose `specversion` is not `1.0`, fails to decode, and
  the hub drops it with a logged warning and no reply, as it does any malformed frame.
- `et-ws-wasm-agent` 0.3.0: `send_client_event` takes the module's `source` as its first argument, which
  `et_org::served_npm_module_path!()` supplies.
- The WebSocket protocol is version 0.2.0, in the AsyncAPI spec and in the `et:ws-messages` and `et:ws-wasi` WIT
  packages. A WASI module built against 0.1.0 no longer instantiates in `et-ws-wasi-runner`; rebuild it.
- `et-ws-wasi-runner` runs on wasmtime 49, through `wasi-webgpu-wasmtime` 0.4.
- Every published crate takes a minor version bump for the protocol change: `edge-toolkit` 0.4.0, `et-ws-server`
  0.3.0, and the rest to their next minor version.

### Added

- `edge_toolkit.events.client_event` builds the envelope for Python modules.
- The `edge-toolkit` Python package and the `et-ws` package it depends on are published to PyPI, through the
  `publish-python-packages` maintainer task.

## [0.2.1] - 2026-10-01

### Added

- Scenario inputs can name external modules for mise deployments; Docker Compose and k3s deployments include them
  and their build contexts, and an unsupported deployment type reports a clear error (#135).
- A valid, safe agent id can be assigned directly; an invalid one falls back to a generated id (#135).
- Published deployments configure persistent agent storage (#135).
- README badges for the test workflow, coverage and the crate listing (#134).

### Changed

- Version bumps for the storage and WebSocket service crates (#136).

## [0.2.0] - 2026-09-24

### Added

- Runner deployments: generated deployments launch headless web, WASI and Python runners and wait for hub
  readiness, with a container image for each runner (#125, #127).
- WASI math1 sender and twin modules, with WASI and pyo3-math1 verification scenarios (#127).
- k3s deployment output for verification scenarios, with hardened non-root runtime configuration and stored-model
  verification for math1 (#128).
- `et-cli npm-module-path`, and scenario-specific module images for Docker deployments (#124).
- Deployment generation from released artifacts as well as local builds, across mise, Docker Compose and k3s, with
  multi-architecture service images (#131).
- Modules published to GitHub's npm registry and consumed from there (#132, #133).
- Minimal CI for tier 3 platforms (#129).
- Duplicate-code checking, with connection timeouts and shared helpers for the browser WebSocket workflows (#121).

### Changed

- Scenario deployments generate reproducible, policy-compliant credentials (#124).
- Linux package prerequisites come from mise bootstrap packages (#122); mise bumped to 2026.9.0 (#123).
- Dependency bumps and higher test coverage (#130).

## [0.1.0] - 2026-09-02

### Added

- First release: the WebSocket hub (`ws-server`) with its agent registry, per-agent storage and module serving; the
  browser WASM agent; and node module packages in Rust, JavaScript, Dart, Kotlin, Python, C#, Java, R and Zig, plus
  WASI and native-Python runners.
