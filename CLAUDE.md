# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Prerequisites

Install [`mise`](https://mise.jdx.dev/) with shell integration, then configure:

```bash
mise settings experimental=true
mise settings set cargo.binstall true
mise install
```

## Common Commands

All tasks run through `mise run <task>`.

| Task                               | Command                       |
| ---------------------------------- | ----------------------------- |
| Format all                         | `mise run fmt`                |
| Check all (lint, format, security) | `mise run check`              |
| Run all tests                      | `mise run test`               |
| Build all WASM modules             | `mise run build-modules`      |
| Run Rust tests only                | `mise run cargo-test`         |
| Run Python tests only              | `mise run test-pyface1`       |
| Run WebSocket server               | `mise run ws-server`          |
| Start OpenObserve (Docker)         | `mise run o2`                 |
| Download ONNX models               | `mise run download-models`    |
| E2E tests (Chrome)                 | `mise run ws-e2e-chrome`      |
| Regenerate verification outputs    | `mise run regen-verification` |

**Rust formatting uses nightly:** `cargo +nightly fmt`

**Build a single module:** `mise run build-ws-<module>-module` (e.g., `build-ws-face-detection-module`)

## Architecture

This is a WebSocket-based edge computing framework.

The server is a hub: it maintains an agent registry, routes messages between agents, provides agents with storage,
and serves node module packages as static files to browsers.

### Services (`services/`)

- **ws-server** — Actix-web entry point; wires together the four concerns below. Loads registry from
  `registry.yaml` on startup and saves it on shutdown. Run with `mise run ws-server`.
- **ws** (`et-ws-service`) — Agent registry and WebSocket hub. The registry tracks agent state and
  queues pending direct messages. Registry is persisted to `registry.yaml`.
- **storage** (`et-storage-service`) — File storage for agents, mounted into the app as configured by `StorageConfig`.
- **modules** (`et-modules-service`) — Scans configured paths for directories containing `package.json`,
  then serves those files statically. The root module (default UI) is the package named by `ModulesConfig::root`.
- **ws-wasm-agent** — Browser-side WASM client that connects back to the server over WebSocket.

### Modules (`services/ws-modules/`)

Node module packages served as static files by `et-modules-service`. Each module has a `package.json`
with a `main` JS entry point. Built artifacts land in `pkg/`. The browser loads and runs these.
The server only serves them from disk.

Languages:

- **Rust → WASM** (wasm-pack): audio1, bluetooth, comm1, data1, face-detection, geolocation, graphics-info, har1, nfc,
  sensor1, speech-recognition, video1
- **Dart → JS**: dart-comm1
- **Python (Pyodide)**: pydata1, pyface1
- **C# (.NET WASM)**: dotnet-data1
- **Java (TeaVM → JS)**: java-data1
- **Zig → WASM**: zig-data1

### Libraries (`libs/`)

- **edge-toolkit** — Common utilities, config, serialization (shared across services)
- **web** — WASM web helpers (Canvas, MediaStream, WebSocket bindings for browser modules)

### Utilities (`utilities/`)

- **cli** (`et-cli`) — Deployment generator: reads scenario YAML, outputs `mise.toml` or `compose.yaml`
- **onnx** — ONNX model utilities
- **module-manifest-to-package-json** — Generates `pkg/package.json` from module metadata.
  Reads `pyproject.toml` (Python modules, via `[tool.ws-module]`)
  or `Cargo.toml` (Rust modules, via `[package.metadata.ws-module]`).

### Verification (`verification/`)

Scenario YAML inputs live in `verification/*/input/`. Expected outputs are checked into `verification/*/output/`
and must stay in sync — `mise run check` will fail if they drift. Regenerate with `mise run regen-verification`.

## Module Build Details

- Most Rust modules: `wasm-pack build . --target web` from the module directory
- WASM agent (nightly, MVP target): uses `RUSTFLAGS="-C target-cpu=mvp ..."` and `RUSTUP_TOOLCHAIN=nightly`
- `har1` and `face-detection`: after wasm-pack, merge extra `package.json` fields with `yq`
- Python modules: `uv build --wheel` then `cargo run -p module-manifest-to-package-json`
- Rust modules needing dependency injection: `cargo run -p module-manifest-to-package-json`
  merges `[package.metadata.ws-module.dependencies]` from `Cargo.toml` into `pkg/package.json`
- Java: `mvn package` from repo root (uses `pom.xml`)

## Observability

The ws-server sends OpenTelemetry traces/logs via OTLP. In development, OpenObserve runs locally:

```bash
mise run o2          # starts Docker container on :5080
mise run open-o2     # opens browser UI
```

Dev credentials: `root@example.com` / `1234` (set in `[tasks.ws-server.env]`).

## Testing

Tests must live in a `tests/` directory or in source files prefixed `test_`.
Do not use inline `#[cfg(test)]` modules.
If a function is private but needs testing, add a `[lib]` target to the crate and export it so `tests/` can reach it.

## Rust Workspace

Single Cargo workspace (`Cargo.toml`).
Shared dependency versions are declared in `[workspace.dependencies]`.
Add new deps there, not in individual crate `[dependencies]`.
