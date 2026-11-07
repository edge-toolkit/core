# Changelog

All notable changes to this project are recorded here, one section per version tag. The format follows Keep a
Changelog 1.1.0.

## [Unreleased]

### Added

- `et-ws-wasi-runner` runs Wasm 1.0 core modules beside WASI components, on NASA's SpaceWasm interpreter, with its
  new `core-modules` feature, on by default. It picks the engine from the fetched module's header, so a scenario's
  `runner: wasi` covers both. A core module reaches the hub through the `et` imports, which the new `et-core-guest`
  crate defines and wraps in a typed guest API for the `wasm32v1-none` target; `core-math1` is the first such module,
  and `verification/local/input/core-math1.yaml` pairs it with the WASI trigger. A runner built without the feature
  fails a core module with `RunnerError::CoreModulesDisabled`.
- A board runs Wasm 1.0 core modules too. `et-ws-wasi-runner`'s `board::run_core_module` hosts one's `et` imports on
  the board's `Platform` and `Transport`, its memory drawn from the same `MemoryBudget` as a component's, and
  `core_module::is_core_module` tells one from a component or a precompiled guest by its header. `board::run_guest`
  runs a guest of either kind, rerunning it after `board::RERUN_DELAY` when it fails in itself. `RUNNER_GUESTS` may
  name a core module for et-ws-server-firmware, whose build checks its imports against the `et` functions, now listed
  as `et_core_guest::abi::IMPORTS`, and builds it in as its package holds it, with nothing compiled ahead of time.
  `core-math1`'s `firmware-pico-plus2` deployment now builds.
- `et-math1` holds the math1 family's `FedAvg` kernel, input pointer and output JSON, which the Rust math1 twins and
  triggers now share rather than each carrying a copy.
- `generated/specs/rest.md` and `generated/specs/ws.md` document the REST API and the WebSocket protocol in Markdown:
  every operation, message and shared schema, with the side of the hub that sends each message. They are rendered
  from `rest.yaml` and `ws.yaml` and regenerated with them.
- `ws.yaml`'s `sendWsMessage` and `receiveWsMessage` operations list the messages each carries, and its `ws` channel
  lists all of them, so AsyncAPI tools see which side sends what.
- `edge-toolkit` builds as `no_std` + `alloc` with `default-features = false`, keeping the WebSocket protocol types
  in `ws`, `ports`, the `BasicAuth` struct and the in-memory agent registry of `ws_server`. The new default `std`
  feature restores everything else, the registry file included. `et-org` is `no_std` unconditionally.
- `et-ws-service` builds as a `no_std` + `alloc` hub with `default-features = false`: `hub::Link` routes each agent's
  frames through the registry and hands back what to send where, leaving the transport, ids and clock to the
  embedder. Its features `std` (the actix-web hub), `nats` (the backplane) and `otel` (spans and metrics) are
  separate. Without `nats`, `WsConfig` has no `backplane`.
- `et-ws-service`'s `board` feature, on by default, adds `picoserve_hub::Hub`: a `no_std` hub serving
  `GET /ws` over HTTP and WebSocket through picoserve on any socket the embedder accepts, such as an `embassy-net`
  `TcpSocket`. It is a local hub, with no backplane, admin API, idle timeout or registry file. A crate awaiting
  `Hub::serve` needs `#![recursion_limit = "256"]`.
- `picoserve_hub::Hub::local` joins an agent the embedder runs itself to the hub, with no socket between them: a
  `LocalLink` routes what it sends as a connection's frames are, queues what others send it, and disconnects the agent
  when dropped. It is how a board's own runner reaches the hub beside it. Its `send_text` and `send_binary` refuse a
  frame larger than the hub reads from a socket with `FrameTooLarge`.
- `et-modules-service` and `et-storage-service` build as `no_std` + `alloc` without their new default `std` feature,
  and each serves through picoserve under its new `board` feature. `et_modules_service::picoserve::ModuleRoutes`
  serves the modules a `Modules` builds into the binary under `/modules` and `/`, as the `std` service serves them
  from disk, redirecting a module listed as served elsewhere to a base URL. `et_storage_service::picoserve`'s
  `StorageRoutes`, mounted at its `path()`, serves agent storage at `/storage/{agent_id}/{filename}`, held to the `std`
  service's ownership rule against an `AgentRegistry` and kept in the embedder's `Storage`, each file at most
  `MAX_STORED` bytes. Both are used with `default-features = false` from the workspace, so a consumer of the `std`
  service now names the `std` feature.
- The `et-ws-server-firmware` firmware runs the hub on a Raspberry Pi Pico 2 W, an ESP32-C6 or a Pimoroni Pico
  Plus 2 W, over the board's Wi-Fi. `MISE_ENV=rust mise run build-firmware-pico2w` fetches the Pico's radio blobs and
  builds it for the Pico, `build-firmware-esp32c6` for the ESP32-C6, and `build-firmware-pico-plus2w` for the Pico Plus
  2 W, each for the network `WIFI_NETWORK` and `WIFI_PASSWORD` name, embedding the modules `MODULES_PATHS` names and
  serving `MODULES_ROOT` at `/`. The Pico Plus 2 W makes its 8 MiB of PSRAM the heap, and also runs each wasi runner
  `RUNNER_GUESTS` names beside the hub, its guest compiled into the firmware and its websocket a link into the hub.
  Every board serves agent storage from memory, and the guests' storage buckets are that same storage, so an agent on
  another machine reads what a guest wrote. Built with `HUB_ADDRESS` naming a hub elsewhere, `address:port`, the
  firmware serves nothing and runs its wasi runners against that hub instead, over the board's own sockets.
- `et-cli generate-deployment --output-type firmware-pico2` writes a `mise.firmware-pico2.toml`, `firmware-esp32c6` a
  `mise.firmware-esp32c6.toml`, and `firmware-pico-plus2` a `mise.firmware-pico-plus2.toml`, whose `firmware-flash`
  task builds that firmware with the scenario's modules in it and loads it onto a board, with picotool or espflash.
  The npm packages the modules use, such as onnxruntime-web, are redirected to jsDelivr rather than built in. The
  board's firmware decides which runners it runs: the Pico Plus 2 W runs a scenario's `wasi` runners, and every other
  runner, or a runner with `limits:`, is refused; the other boards refuse any runner. A scenario with `machines:` or
  published artifacts is refused on every board. Its `firmware-host-test` task serves the same modules from the host,
  and runs the same runners, as the firmware's host build does, and fetches every module it lists, with no board
  needed.
- A scenario's machine names the hardware it is with `device:`. A board (`pico2`, `esp32c6` or `pico-plus2`) as the
  first machine runs the hub as that board's firmware, serving every module of the scenario, with the runners its
  firmware hosts beside it, while each other machine runs its share as the format's tasks and reaches the board's hub
  at its `address`. A board on any other machine runs only the runners placed on it, built as firmware that serves
  nothing and reaches the server's hub at `HUB_ADDRESS`. The `mise.toml` gains `firmware-build-<machine>` and
  `firmware-flash-<machine>` tasks for each board, each built in a target directory of its own, a
  `firmware-host-test-<machine>` for a board running the hub, and no collector when the server is a board. Only
  `mise`, `dekit` and `process-compose` deliver such a scenario.
- `et-firmware-modules` generates the source a firmware's build script writes to build ws-modules into its image,
  from the same `MODULES_*` variables, so every board's firmware embeds modules the same way. Its `board` writes what
  the board a target picks bakes in -- the Wi-Fi network, and the Pico 2 W's radio blobs and memory layout -- and
  returns the directives that link it. Its `guests` compiles each runner's guest for the board, refusing one that
  imports what a board does not host, or guests needing more memory than the board holds for them.
- Native REST requests retry transient HTTP 408/429/500/502/503/504 responses as well as transport failures, using a
  bounded backoff policy for replayable idempotent operations. Permission errors, non-idempotent methods and streaming
  request bodies are not retried. Storage-call timings include the retry/backoff interval; retry attempts are logged.
- Recovery supervision reconnects after hub interruptions and restores incident context with explicit observation gaps.
  Optional dependency checks, fleet action limits, bounded backoff with jitter and sustained-recovery assessment extend
  the existing restart policy. Reports distinguish observed failure conditions from reasons for withholding actions.
- `edge-toolkit`'s `board` module lists the interfaces a board's runner hosts, and its `Guest` reads what a component
  imports and how much memory it starts with, so a guest can be checked against a board before it is built in.
- `et-ws-wasi-runner` builds as `no_std` + `alloc` without its new default `std` feature, and with its new `board`
  feature its `board` module runs a guest on a microcontroller: compiled ahead of time with `engine::precompile` (the
  `precompile` feature) for the board's Pulley target, and hosted through the firmware's `board::Platform` and
  `board::Transport`. The transport is the hub as the guest reaches it, its storage included: a board running the hub
  answers from `board::Buckets`, and `board::net::NetTransport` reaches a hub on another machine over the board's own
  sockets, its websocket framed by `edge-ws` and its storage over HTTP, while a task of the board's runs
  `board::net::drive`; `board::net_tokio` gives it tokio's sockets on a host. Every guest draws its memories and
  tables from one `board::MemoryBudget`, which refuses growth past what the board holds for them.
- An opt-in `et-recovery-supervisor` service observes application progress and coordinates bounded Kubernetes recovery.
- The Python SDK provides recovery lifecycle messages and integrity-checked checkpoint storage helpers.
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
- A scenario can publish ports outside a k3s cluster with `expose:`: `services.hub.expose` lists the hub's `ws`
  and `wss`, so a browser can open a k3s deployment's hub, and an agent with a web or pyo3 runner lists the ports
  its module listens on. Each becomes a `LoadBalancer` `Service` that k3s's ServiceLB binds on the machine running
  the process, and the README lists their addresses. A scenario is now rejected, in every format, when two
  processes on one machine would use the same port, including an agent port that is the hub's or the collector's.
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
- A scenario can run a hub on more than one machine: `services.hub.machines` lists them, the server among them. Each
  runner joins the hub on its own machine, or the server's when its machine runs none. Every format then also runs a
  NATS server and a rustfs object store on the server, which the hubs share as their backplane and their agent
  storage, so an upload through one hub reads back through any other. `services.backplane` and `services.storage`
  take `limits:` for the two. `secrets.env` carries the store's secret as `AWS_SECRET_ACCESS_KEY` and
  `RUSTFS_SECRET_KEY`. A scenario without `services.hub.machines` renders as before.
- `WS_MESSAGE_RETENTION=all` makes the hub keep every unacknowledged direct message a recipient has from a sender,
  rather than only the newest. The default, `latest-per-sender`, behaves as before.
- An operator can block an IP address, or evict a misbehaving agent. `PUT /blocked-ips/{ip}` blocks an address: the
  hub closes every socket from it within a second, refuses its WebSocket upgrades, and refuses its `/storage`
  requests with 403. `DELETE /agents/{agent_id}` blocks the address the agent last connected from the same way, and
  drops the direct messages it sent that no recipient has acknowledged yet. With a NATS backplane a block reaches
  every hub through the new `et-blocked` KV bucket, including hubs that join later. A local hub saves it in
  `registry.yaml`. Both routes need `WS_ADMIN_TOKEN` set on the hub and presented as an `Authorization: Bearer`
  token. While the variable is unset, the admin API answers 403. `et-cli block-ip --hub-url <url> --ip <address>` and
  `et-cli evict-agent --hub-url <url> --agent-id <id>` send the requests, reading the token from `WS_ADMIN_TOKEN`.
  There is no unblock yet; remove the address from `registry.yaml` or the KV bucket and restart the hubs.

### Changed

- **Breaking:** The modules moved from `services/ws-modules/` to `modules/<type>/`, the type being the runtime that
  loads them: `web`, `wasi`, `core` or `pyo3`, as `edge_toolkit::config::MODULE_TYPE_DIRS` lists them.
  `default_modules_folders` returns those four in place of `services/ws-modules`, and a generated deployment stages
  each module at `/app/modules/<type>/<name>`. A hub configured with the old path, or a deployment generated before
  this change, must be pointed at the new one or regenerated. The Zig modules' shared build moved to
  `libs/zig-shared`.
- **Breaking:** `et_ws_server::routes::HealthResponse`, the answer to `GET /health`, is now
  `et_ws_service::health::HealthResponse`, since a board's hub gives the same answer; import it from there.
- **Breaking:** `et-ws-wasi-runner`'s host runner, its binary and `run_module` are behind the new default `std`
  feature, which `nn`, `webgpu` and `coverage` imply. A build with `default-features = false` and none of them now
  gets the `no_std` board runner and no binary; add `std` to keep the host runner. `server_message_to_wit` moved from
  `host::ws` to `convert`, and `Bucket` from `host::wasi_keyvalue` and `kv_not_implemented` from `host` to
  `keyvalue`.
- **Breaking:** `et-cli` refuses a scenario whose `module_paths:` entry, or a module directory found under one, has a
  name a shell would need quoted or that some filesystem could not hold, such as `my mod`, `mod$(id)` or `con`; rename
  the directory. Generated deployments no longer escape these paths. The check is the new `et-sanitise` crate, which
  accepts a name only when both sanitize-filename, with its Windows rules, and shlex leave it unchanged.
- **Breaking:** Recovery state is bound to its complete configuration. Changing configuration requires a new state
  location or an explicitly provisioned new job. Legacy reports retain their restart budget and are archived in the
  new context.
- The workspace requires actix-web 4.17.
- **Breaking:** the minimum supported Rust version is 1.99.0, up from 1.91.1, for every published crate.
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
- **Breaking:** `PUT /storage/{agent_id}/{filename}` accepts a write only while the agent is connected, and only
  from the address it is connected from; anything else is answered `403`. An upload made on an agent's behalf must
  now come from the agent's own host while it is connected.
- **Breaking:** `AgentRegistry::queue_direct` and `NatsBackplane::queue` take a `MessageRetention`.
  `Backplane::knows` is replaced by `Backplane::presence`, and `Presence::Connected` carries the agent's
  `last_known_ip`.
- **Breaking:** `registry.yaml` is now a map with `agents` (the former top-level map of agent records) and
  `blocked_ips`. A file in the old layout still loads, and is written back in the new one. The `/storage` `GET`
  and `HEAD` handlers, like `PUT`, now take the agent registry and are generic over its session type, so an app
  serving them must register an `AgentRegistry`.

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
