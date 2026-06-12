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

The `mise` configuration lives under [`.mise/`](.mise/): the always-loaded
[`.mise/config.toml`](.mise/config.toml) holds the Rust/Node tooling and shared
tasks, and per-language `.mise/config.<lang>.toml` files (dart, dotnet, java,
python, zig) are selected via `MISE_ENV` so a dev can work on one language
without installing the others — e.g. `MISE_ENV=dart mise install`. CI runs
every language; `mise run check-all` (and `install-all`, `test-all`, …) act on
all of them at once.

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

If `mise install` fails on Windows with `unknown field run_auto_install`, your
mise is outdated (e.g. 2026.3) — update to a current mise, which reads the
config fine.

On Windows only, `pipx` also needs to be pre-installed.
See the Windows section of [pipx instructions](https://pipx.pypa.io/stable/how-to/install-pipx/).

### MacOS only

On MacOS, the Xcode Command Line Tools (`clang`, `git`, `make`, etc.) must be
installed first:

```bash
xcode-select --install
```

We also need to install a better linker into the workspace.

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
mise install-all
```

## Contributing

Use `mise run fmt-all` and `mise run check-all` to run formatters and checkers.

## Building and running with Docker

[`Dockerfile`](Dockerfile) reproduces the mise setup above on a clean, minimal
Ubuntu, in stages (`build` → `prefetch` → `precompile` → `test`/`server`). A
plain build produces the **server** image (the final stage): a release build of
`et-ws-server`, served automatically. `mise install-all` fetches many tools from
GitHub releases, so build with a GitHub token to avoid the anonymous
60-requests/hour limit (see [GitHub rate limits](#github-rate-limits)), passed as
a BuildKit secret so it never lands in an image layer:

```bash
GITHUB_TOKEN="$(gh auth token)" DOCKER_BUILDKIT=1 \
  docker build --secret id=gh_token,env=GITHUB_TOKEN -t edge-toolkit .
docker run --rm -p 8080:8080 edge-toolkit
```

Then open <http://localhost:8080> (add `-p 8443:8443` for TLS). The server needs
no GPU. OpenObserve/`o2` is optional — OTLP export is off when no collector is
configured. (Drop `--secret` to build without a token; `install-all` may then hit
rate limits.)

The full test suite is a **separate, non-final stage**, so build it explicitly
with `--target test`. The WebGPU compute test needs a GPU, and `docker build`
can't attach one (no `--gpus` for build), so it runs at `docker run` time. The
`test` stage bundles `mesa-vulkan-drivers`, so passing the host DRI node gives
wgpu a real Intel/AMD GPU (and a software fallback if you pass nothing):

```bash
docker build --target test -t edge-toolkit-test .
docker run --rm --device /dev/dri edge-toolkit-test   # Intel/AMD GPU
```

NVIDIA via `--gpus all` (with the NVIDIA Container Toolkit) is wired but
**unverified** — its in-container Vulkan ICD doesn't initialize yet, so prefer a
DRI device. The image skips the `o2`/`ws-server` README steps (runtime services).

### CI

The [`docker`](.github/workflows/docker.yml) workflow rebuilds these images when
a `Dockerfile*` or `.dockerignore` changes (or on manual dispatch) — both builds
are too heavy for every push. A `linux` job builds the `test` stage and runs the
suite (lavapipe software Vulkan, since the runners have no GPU); a `windows` job
builds the sketch below.

### Windows setup sketch

[`Dockerfile.windows`](Dockerfile.windows) is an **unverified sketch** that
proves the stronger claim: on a bare Windows box, mise supplies the _entire_
toolchain and nothing is installed the Windows way. It starts from **Nano
Server** (the smallest Windows base, ~120 MB) — which has no installer stack,
PowerShell, or admin shell, so the Visual Studio Build Tools installer can't run
there at all. Instead it bootstraps just `mise.exe` (via the `curl`/`tar` built
into Nano Server) and lets `mise install` pull everything else: rust, an
`llvm-mingw` toolchain (clang + lld + the mingw-w64 runtime and `libclang.dll`),
plus `bash` and `git` from conda's msys2 packages — all declared os-guarded in
[`.mise/config.toml`](.mise/config.toml). Because the MSVC CRT + Windows SDK are
the one thing mise can't supply, the build targets `x86_64-pc-windows-gnu`
rather than `-msvc`. Windows containers build only on a Windows Docker host, so
the `windows` CI job is the only place it runs; expect to iterate there (the
rustup gnu-host flip and whether msvc-only prebuilts like `ort` link are the
open questions). Whatever it ends up needing are findings the README's "Windows
only" section should fold in.

## Run ws agent in browser

### Build modules and run the WS server

In a separate terminal start OpenObserve (o2) and leave it running.

```bash
mise run o2
```

Then start the fetch the ONNX models and run the server

```bash
mise run download-models
mise run build-modules-all
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
`mise run gen-specs-all`:

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
