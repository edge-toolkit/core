# et-ws-wasi-runner

Runs edge-toolkit ws-modules compiled to **WASI Preview 2 components**, rather than browser WASM modules. It fetches
the module's `pkg/package.json` from the ws-server, downloads the `.wasm` named by the `wasi-main` field,
instantiates it under `wasmtime` with async support, and calls the exported `entry.run` function.

```sh
RUNNER_MODULE=@edge-toolkit/et-ws-wasi-math1 WS_SERVER_URL=ws://localhost:8080/ws et-ws-wasi-runner
```

`RUNNER_MODULE` names the module as the ws-server serves it.
[HELP.md](https://github.com/edge-toolkit/core/blob/main/services/ws-wasi-runner/HELP.md) lists every environment
variable the runner reads, with its type and default.

## Host imports

Defined in `generated/specs/wit/world.wit` of the edge-toolkit repository, package `et:ws-wasi@0.1.0`:

- `log` -- `log` and `set-status` for guest output
- `clock` -- `sleep-ms`, `now-ms`
- `storage` -- `put-file`/`get-file` proxied to the ws-server's storage service via reqwest
- `ws` -- websocket client backed by `tokio-tungstenite`; mirrors the wire format of `et-ws-wasm-agent` so events
  look the same on the server

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
