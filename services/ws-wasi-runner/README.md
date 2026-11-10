# et-ws-wasi-runner

Runs edge-toolkit ws-modules built as WebAssembly for a host rather than for a browser. It fetches the module's
`pkg/package.json` from the ws-server, downloads the `.wasm` its `main` field names, and picks the engine from the
module's header: a **WASI Preview 2 component** is instantiated under `wasmtime` with async support and its exported
`entry.run` called, and a **Wasm 1.0 core module** runs on NASA's SpaceWasm interpreter, which calls its exported
`run`.

```sh
RUNNER_MODULE=@edge-toolkit/et-ws-wasi-math1 WS_SERVER_URL=ws://localhost:8080/ws et-ws-wasi-runner
```

`RUNNER_MODULE` names the module as the ws-server serves it.
[HELP.md](https://github.com/edge-toolkit/core/blob/main/services/ws-wasi-runner/HELP.md) lists every environment
variable the runner reads, with its type and default.

## Host imports

A component imports the `runner` world in `generated/specs/wit/world.wit` of the edge-toolkit repository:

- `et:ws-wasi/ws` -- websocket client backed by `tokio-tungstenite`; mirrors the wire format of `et-ws-wasm-agent` so
  events look the same on the server
- `wasi:keyvalue/store` -- the agent's own storage bucket, and a module's files read-only, proxied to the ws-server's
  storage and modules services through its REST API
- `wasi:logging/logging` -- the guest's log, routed into `tracing`

Plus, attached to the same `Linker` but defined by external WIT packages, each behind the cargo feature named first:

- `webgpu`: `wasi:webgpu/webgpu@0.3.0-rc.2` -- the full upstream interface, implemented by the
  `wasi-webgpu-wasmtime` crate from the wasi-gfx project. The runner supplies only a `wgpu_core::global::Global`
  handle and the resource table, via `WasiWebGpuCtxView` on `HostState`; everything else (every resource, every
  method, render paths included) is upstream's. The WIT is fetched verbatim into
  `generated/specs/wit/deps/wasi-webgpu/` by `fetch-wit-deps`, not trimmed. This interface is where the `async` on
  `entry.run` comes from: `request-adapter`, `request-device` and `map-async` are `async func`, a component can only
  await an async import from an async export, so every WASI guest's entrypoint carries the async ABI and the host
  drives it through `Store::run_concurrent`.
- `nn`: `wasi:nn/{tensor, graph, inference, errors}` -- standardised ML inference. The host wires
  `wasmtime-wasi-nn` with the ONNX Runtime backend (`ort` 2.0.0-rc.10, pinned because rc.11+ moved API surface that
  wasmtime-wasi-nn 48 still uses). Guests load model bytes via `graph.load`, build `Tensor`s, and call `compute` --
  the same shape of calls Spin / wasmCloud / Fermyon production wasi-nn workloads use. Inference runs on the CPU
  unless the `cuda` feature is on.
- `lidar`: `et:ws-wasi/serial` -- the serial ports the runner's `SERIAL_PORTS_<NAME>` variables lend a guest, each a
  device the `serialport` crate opens or a port a bridge such as ser2net serves over TCP. It is this repository's own
  interface, in the `serial-host` world rather than `runner`, so that its host bindings link only in a build with the
  feature; no WASI proposal covers serial ports. It is how the `wasi-lidar1` module reads a lidar, the COIN-D6 on
  Hiwonder's MentorPi robot, keeping the latest revolution in its agent's storage:

  ```sh
  export SERIAL_PORTS_LIDAR=/dev/ttyUSB0 RUNNER_MODULE=@edge-toolkit/et-ws-wasi-lidar1
  WS_SERVER_URL=ws://hub:8080/ws et-ws-wasi-runner
  ```

  On the MentorPi, Hiwonder's ROS 2 lidar driver holds that port while it runs, so stop it first.

## Core modules

The `runner` world exists only through the component model, so a core module cannot import it. With the
`core-modules` feature, on by default, the runner runs one on [SpaceWasm](https://github.com/nasa/spacewasm), NASA
JPL's Wasm 1.0 interpreter for flight software, in place of wasmtime. SpaceWasm runs Wasm 1.0 with mutable globals
and nothing later, which is what Rust's `wasm32v1-none` target builds; a `wasm32-wasip1` or `wasm32-wasip2` build
uses bulk memory, sign extension, multi-value and reference types, which it rejects. A core module imports the same
operations as the world -- the websocket, the storage buckets and the log -- flattened into the `et` functions that
`et-core-guest` defines, and `core-math1` is one.

An import that has to wait on the hub pauses the interpreter. The call is carried out on the same `HostState` a
component's imports are, and the guest resumed with its result; between pauses the interpreter runs in slices of
instructions, yielding between them. SpaceWasm's engine cannot move between threads, so the module runs on a thread
of its own. The `core_module` module is `no_std` + `alloc` and runs on any `CoreHost`, which is how a board hosts a
core module too.

## On a board

Without its default `std` feature the crate is `no_std` + `alloc`, and with its `board` feature its `board` module
runs a guest on a microcontroller with no operating system. A board cannot compile WebAssembly, so its engine runs
Pulley bytecode instead: a firmware's build script compiles the guest ahead of time with `engine::precompile` (the
`precompile` feature) for the board's Pulley target, `pulley32` on a 32-bit board, and builds the result in.

`board::run` hosts the `runner` world on what the firmware provides through two traits: a `Platform`, the board's
clock and log, and a `Transport`, its websocket to the hub. Guest buckets live in the board's memory, in the
`Buckets` the firmware hands every guest it runs, so one reads what another wrote. Their memories and tables draw on
the one `MemoryBudget` the firmware hands them too: growth past what is left fails in the guest, as `memory.grow`
returning -1, rather than the board running out of heap, and what a guest held is returned when it ends. Of WASI
Preview 2 it hosts only
what the ws-module guests import -- their standard streams, an empty environment, the monotonic clock and polling --
since `wasmtime-wasi` needs `std`; `wasi:nn`, `wasi:webgpu` and `et:ws-wasi/serial` are host-only.

With `core-modules`, `board::run_core_module` runs a core module on SpaceWasm with its `et` imports hosted on the same
`Platform` and `Transport`. SpaceWasm decodes the module as it is, so a firmware builds one in with nothing compiled
ahead of time. Its linear memory draws on the `MemoryBudget` as a component's memories do. `board::run_guest` is what
a firmware runs each guest through: it picks the engine by the guest's header, as the host runner does, and supervises
the guest as a process manager would a runner.

wasmtime asks the firmware for one thing more, two thread-local pointers through `wasmtime_tls_get` and
`wasmtime_tls_set`, which on a single core are statics. A guest's linear memory is its declared size, about 1 MiB
for the Rust guests, so a board needs external RAM such as PSRAM to hold it: et-ws-server-firmware runs guests on the
Pimoroni Pico Plus 2 W, whose PSRAM is its heap.
