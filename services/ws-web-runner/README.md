# et-ws-web-runner

Runs browser-targeted edge-toolkit ws-modules natively, under embedded Deno. It is the counterpart to
`et-ws-wasi-runner`, which runs WASI components inside wasmtime; this crate runs the JavaScript entry points
(wasm-bindgen glue, Pyodide shims, Dart/Zig/Java shims) that normally load in a real browser.

The runner fetches `package.json` from the ws-server, downloads the `main` JS file, and evaluates it inside a Deno
`JsRuntime` equipped with the standard web platform extensions (fetch, `WebSocket`, `WebStorage`, timers, crypto,
WebGPU).

```sh
RUNNER_MODULE=@edge-toolkit/et-ws-math1 WS_SERVER_URL=ws://localhost:8080/ws et-ws-web-runner
```

[HELP.md](https://github.com/edge-toolkit/core/blob/main/services/ws-web-runner/HELP.md) lists every environment
variable the runner reads, with its type and default.
