# edge-toolkit core

edge-toolkit is a WebSocket-based edge-computing framework that runs AI on hardware you control,
so nothing has to leave your network. A lightweight server acts as a hub that serves small AI
modules — written in Rust, Python, Dart, C#, Java and more, each compiled to WebAssembly or
transpiled to JavaScript — straight to a browser, where they run locally and can reach the
browser's own Web APIs (camera, microphone, geolocation, motion sensors, Bluetooth, NFC) to sense
the real world directly. The same framework also drives larger models on local GPU hardware through
standardised WebAssembly interfaces, so one toolkit spans on-device and server inference without
changing the protocol.

The result is AI that protects privacy and data sovereignty: sensitive camera, audio and research
data stay on the device or your own network, never sent to an external cloud service.

## mise

Please install [`mise`](https://mise.jdx.dev/), including the shell integration.
It is needed for all use of this repository.

The `mise` configuration is stored in [`.mise.toml`](.mise.toml).

The following works for Linux, macOS and Windows, and all tools "installed"
are only installed into the local workspace, so no need for admin/root privileges.

Configure it with:

```bash
mise settings experimental=true
mise settings set cargo.binstall true
```

Pre-install `cargo-install`, which can be done using:

```bash
mise use -g cargo-binstall
```

### Windows only

On Windows only, `pipx` also needs to be pre-installed.
See the Windows section of [pipx instructions](https://pipx.pypa.io/stable/how-to/install-pipx/).

### MacOS only

On MacOS, we need to install a better linker into the workspace.

```bash
mise install conda:lld
```

### All OS

Before installing dependencies, please the install openssl development files
separately:

```bash
mise install conda:openssl
```

Then install the remaining dependencies:

```bash
mise install
```

## Contributing

Use `mise run fmt` and `mise run check` to run formatters and checkers.

## Run ws agent in browser

### Build modules and run the WS server

In a separate terminal start OpenObserve (o2) and leave it running.

```bash
mise run o2
```

Then start the fetch the ONNX models and run the server

```bash
mise run download-models
mise run build-modules
mise run ws-server
```

Scan the QR-Code with a smart-phone camera and open the URL.

Select the module to run in the drop-down, then click "Run module" button.

Note: The WASM build disables WebAssembly reference types, so it can still load on older browsers such as Chrome 95.

In a separate terminal, open the OpenObserve UX using:

```bash
mise run open-o2
```

The server logs appear in the Logs section.

## Modules

The module list is dynamically populated from the modules in [services/ws-modules](services/ws-modules).

Each module must have a `package.json` that defines a `main` which contains a JavaScript file
that can load and run the module.

Under each module in `ws-modules`, the package can be found in a subdirectory `pkg`.

Most of the module are built from Rust using `wasm-pack build --target web`.

There are also modules written in:

- Dart
- Java
- .Net C#
- Python, using [pyodide](https://pyodide.org/)
- Zig, including C code.

## Root module

The default UX in the web-browser is also a loadable module located in
[services/ws-server/static](services/ws-server/static).

A custom UX module can be used by setting the `ws-server` environment variable `MODULES_ROOT`.

## Protocol & API specs

The WebSocket protocol and the ws-server REST surface are described by
machine-readable specs regenerated from their Rust sources of truth by
`mise run gen-specs`:

- **WebSocket** (AsyncAPI 3.0): [`generated/specs/ws.yaml`](generated/specs/ws.yaml).
  Source: `ClientMessage` + `ServerMessage` in `libs/edge-toolkit/src/ws.rs`. Generated clients:
  [`generated/dart-ws/`](generated/dart-ws/),
  [`generated/python-ws/`](generated/python-ws/), and the
  `et:ws-messages` WIT under `generated/specs/wit/deps/`.
- **REST** (OpenAPI 3.0): [`generated/specs/rest.yaml`](generated/specs/rest.yaml).
  Source: `#[utoipa::path]` annotations on the handlers in
  `services/{ws-server,modules,storage}`. Typed Rust client at
  [`generated/rust-rest/`](generated/rust-rest/) (consumed by
  `et-ws-wasi-runner` and the browser `data1` module).

See [`generated/README.md`](generated/README.md) for a full catalogue
of what's regenerated vs. hand-maintained under `generated/`.

## Run e2e

Run the end-to-end tests using Chrome:

```bash
mise run ws-e2e-chrome
```

Run an example demo scenario using et-cli

```bash
cargo install --path utilities/cli --force
et-cli generate-deployment \
  --input-file verification/local/input/facility-security-scenario.yaml \
  --output-dir verification/local/output/facility-security-scenario
```

This will generate a `mise.toml` file under
`verification/local/output/facility-security-scenario`. Run the following
command to start the demo scenario:

```bash
mise run generated-scenario
```

To generate a Docker Compose deployment instead, pass
`--output-type docker-compose` or set `deployment_type: docker-compose` in the
scenario input YAML. This writes `compose.yaml` to the output directory:

```bash
et-cli generate-deployment \
  --input-file verification/local/input/facility-security-scenario.yaml \
  --output-dir verification/local/output/facility-security-scenario \
  --output-type docker-compose
cd verification/local/output/facility-security-scenario
docker compose up --build
```

The generated scenario config only selects which prebuilt modules `ws-server`
serves. Module builds are expected to be handled separately from the repository
root.

To regenerate all checked-in verification outputs from
`verification/*/input`, writing each scenario to
the matching `verification/*/output/<input-file-stem>` folder. This generates
all supported deployment files for each scenario, currently `mise.toml` and
`compose.yaml`:

```bash
mise run regen-verification
```

## Grant

This repository is part of a grant managed by the School of EECMS, Curtin University.

```text
ABN 99 143 842 569.

CRICOS Provider Code 00301J.

TEQSA PRV12158
```
