# edge-toolkit core

edge-toolkit is a WebSocket-based edge-computing framework that runs AI on hardware you control, so nothing has to leave
your network. A lightweight server acts as a hub that serves small AI modules -- written in Rust, Python, Dart, C#,
Java and more, each compiled to WebAssembly or transpiled to JavaScript -- straight to a browser, where they run locally
and can reach the browser's own Web APIs (camera, microphone, geolocation, motion sensors, Bluetooth, NFC) to sense the
real world directly. The same framework also drives larger models on local GPU hardware through standardised WebAssembly
interfaces, so one toolkit spans on-device and server inference without changing the protocol.

The result is AI that protects privacy and data sovereignty: sensitive camera, audio and research data stay on the
device or your own network, never sent to an external cloud service.

## mise

Please install [`mise`](https://mise.jdx.dev/) (2026.7.1 or later), including the shell integration. It is needed
for all use of this repository.

The `mise` configuration lives under [`.mise/`](.mise/): the always-loaded [`.mise/config.toml`](.mise/config.toml)
holds the Rust/Node tooling and shared tasks, and per-language `.mise/config.<lang>.toml` files are selected via
`MISE_ENV` so a dev can work on one language without installing the others -- e.g.
`MISE_ENV=dart mise install`. CI runs every language; `mise run check-all` (and `install-all`, `test-all`, ...) act
on all of them at once.

The following works for Linux, macOS and Windows, and all tools "installed" are only installed into the local workspace,
so no need for admin/root privileges.

Then run the preinstall task for your platform -- it configures mise and installs the shared basics (cargo-binstall,
node, the openssl dev files) plus whatever that platform needs. Do any manual prerequisites in your platform's section
below first.

### GitHub rate limits

`mise install` downloads many tools from GitHub releases, which are subject to
[GitHub's REST API rate limits](https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api).
Unauthenticated requests share a low per-IP limit, so installs can fail with
`GitHub rate limit exceeded`. Authenticate with the
[`gh` CLI](https://cli.github.com/) and let `mise` reuse its token -- this raises
the limit and needs no token scopes:

```bash
mise install gh
```

```bash
gh auth login                                        # any method; no scopes needed
mise settings set github.credential_command "gh auth token"
mise token github                                    # verify: should resolve a token
```

If you havent activated mise shell integration, use this instead, however it is brittle:

```bash
mise settings set github.credential_command "$(mise which gh) auth token"
```

`gh` often stores the token in the OS keyring rather than in `~/.config/gh/hosts.yml`, so mise's default `hosts.yml`
lookup finds nothing; `credential_command` asks `gh` for the token on demand and works either way. The setting is
written to your global `~/.config/mise/config.toml`, so it stays per-machine and out of the repo.

#### Windows (scoop): mise, git, and the gh credential command

On Windows this project is set up with [scoop](https://scoop.sh/) as the package manager -- install both
`mise` and `git` through it. `git` is needed for more than version control here: it ships the real `bash.exe`
that the credential-command steps below depend on, and `scoop prefix git` resolves to that install:

```powershell
scoop install mise git
```

The repo sets `windows_default_inline_shell_args = "bash -euo pipefail -c"` (its tasks need bash), and mise
runs `github.credential_command` **through that inline shell** -- with its own shims stripped from `PATH`, and
ignoring `MISE_BASH_PATH` (that override only applies to task execution, not credential commands). The repo's
only bash is busybox `ash.exe`, reached solely via `MISE_BASH_PATH`, so a literal `bash` is not found: the
credential command fails silently and installs fall back to unauthenticated, rate-limited requests.

Two per-machine settings fix it -- the repo's `bash` inline-shell setting stays untouched:

```powershell
# 1. Put a real bash on PATH via a single scoop shim, pointing at the bash that ships with scoop's git.
#    Nothing else from git lands on PATH, and scoop's shims dir is on PATH and is not stripped by mise.
scoop shim add bash "$(scoop prefix git)\bin\bash.exe"

# 2. Point credential_command at gh's ABSOLUTE path with FORWARD slashes: inside `bash -c` a backslash is
#    an escape character, so a `C:\...` path would be mangled and gh would not be found.
mise settings set github.credential_command "$((mise which gh) -replace '\\','/') auth token"
```

`mise token github` should then resolve a token reported as `source: credential_command`.

### Microsoft VC++ runtime

mise.exe links the Microsoft VC++ runtime (`vcruntime140.dll`), so it must be present or mise won't start. It's
preinstalled on Windows 10/11 and Server, so you already have it -- only Nano Server omits it, and there the
Docker build installs the [VC++ Redistributable](https://aka.ms/vs/17/release/vc_redist.x64.exe).

mise-powered builds use the `x86_64-pc-windows-gnullvm` Rust target (llvm-mingw), which needs no MSVC toolchain or
Windows SDK on disk. Two opt-in target envs retarget the native build: `MISE_ENV=mingw` switches to
`x86_64-pc-windows-gnu` with the winlibs mingw-w64 GCC toolchain installed by mise, and `MISE_ENV=msvc` switches
to `x86_64-pc-windows-msvc` with a portable MSVC compiler + Windows SDK staged by `mise run prefetch:msvc` -- no
Visual Studio install or admin rights needed for either.

### Windows shell

On Windows, install the shell:

```bash
mise install http:busybox
```

### All OS

To install dependencies:

```bash
mise run preinstall
mise install-all
```

The `preinstall` task will advise if there are any required dependencies are are missing, such as Xcode Command
Line Tools on MacOS.

### Install failures

`mise install` runs tool installs in parallel. If they fail intermittently -- a download race, or a `cargo:` source
build colliding with another -- serialize them with `MISE_JOBS=1`:

```bash
MISE_JOBS=1 mise install-all
```

This is the same workaround both Docker builds bake in, so reach for it first if a local install or build misbehaves.

## Contributing

Use `mise run fmt-all` and `mise run check-all` to run formatters and checkers.

## Building and running with Docker

[`Dockerfile`](Dockerfile) reproduces the mise setup above on a clean, minimal Ubuntu, in stages (`build` ->
`prefetch` -> `precompile` -> `test`/`server`, plus a CI-only `check` stage at the end). Build the **server** image
with `--target server`: a release build of `et-ws-server`, served automatically. `mise install-all` fetches many
tools from GitHub releases, so build with a GitHub token to avoid the anonymous rate limit (see
[GitHub rate limits](#github-rate-limits)), passed as a BuildKit secret so it never lands in an image layer:

```bash
GITHUB_TOKEN="$(gh auth token)" DOCKER_BUILDKIT=1 \
  docker build --target server --secret id=gh_token,env=GITHUB_TOKEN -t et-ws-server .
docker run --rm -p 8080:8080 et-ws-server
```

Then open <http://localhost:8080> (add `-p 8443:8443` for TLS). The server needs no GPU. OpenObserve/`o2` is
optional -- OTLP export is off when no collector is configured. (Drop `--secret` to build without a token;
`install-all` may then hit rate limits.)

The full test suite is a **separate, non-final stage**, so build it explicitly with `--target test`. The WebGPU
compute test needs a GPU, and `docker build` can't attach one (no `--gpus` for build), so it runs at `docker run`
time. The `test` stage bundles `mesa-vulkan-drivers`, so passing the host DRI node gives wgpu a real Intel/AMD GPU
(and a software fallback if you pass nothing):

```bash
GITHUB_TOKEN="$(gh auth token)" DOCKER_BUILDKIT=1 \
  docker build --target test --secret id=gh_token,env=GITHUB_TOKEN -t et-test .

docker run --rm --device /dev/dri et-test   # Intel/AMD GPU
```

NVIDIA via `--gpus all` (with the NVIDIA Container Toolkit) is wired but **unverified** -- its in-container Vulkan
ICD doesn't initialize yet, so prefer a DRI device. The image skips the `o2`/`ws-server` README steps (runtime
services).

The base image is parameterised -- `--build-arg BASE_IMAGE=debian:trixie` (or `fedora:42`, `ubuntu:26.04`,
`debian:bookworm`, etc.) builds against a different distro instead of the default `ubuntu:24.04`. The apt step
picks an `apt-get` or `dnf` install path by `command -v`, and the libicu runtime package .NET needs is
auto-detected per base. The CI matrix in [`docker-linux`](.github/workflows/docker-linux.yaml) exercises every
supported base on each PR.

**Only Ubuntu is actively maintained.** Debian and Fedora bases are tested on
every PR (CI matrix) but treated as best-effort -- breakage on those lanes will be triaged but not blocking. If you ship
on Debian or a RPM distro and want first-class support, please open an issue.

**WSL Ubuntu images are supported** as a first-class Linux target -- the mise preinstall and Linux Dockerfile path
work unchanged inside WSL's Ubuntu distributions (`Ubuntu-22.04`, `Ubuntu-24.04`, `Ubuntu-26.04`). Docker Desktop
on Windows with the WSL2 backend runs the Linux image; native Linux Docker under WSL itself does too.

### CI

The [`docker-linux`](.github/workflows/docker-linux.yaml) and [`docker-windows`](.github/workflows/docker-windows.yaml)
workflows rebuild these images when their respective `Dockerfile` is modified.

[`Dockerfile.nanoserver`](Dockerfile.nanoserver) starts from **Nano Server** (the smallest Windows base, ~120 MB)
-- which has no installer stack, or shell. Python and .Net do not work on this base.

[`Dockerfile.windows`](Dockerfile.windows) is the **Server Core** variant (~1.25 GB base) -- the fuller Windows
image, and every language installs. The one gap: the C# -> WASM module build (dotnet-data1) skips on every Windows
host (Server Core and native windows-latest alike), so that module's `pkg/` isn't produced and its integration
test logs a skip.

A `gh_token` file (a GitHub token) in the build context is **optional** for a manual `Dockerfile.nanoserver` build
-- without it, mise uses GitHub's anonymous rate limit and may be throttled. The classic Windows builder has no
BuildKit secrets, so it **bakes that file into an image layer**: never publish an image built with a `gh_token`.
(The Linux build passes the token as a BuildKit secret instead, so it never lands in a layer.)

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

Each module must have a `package.json` that defines a `main` which contains a JavaScript file that can load and run
the module.

Under each module in `ws-modules`, the package can be found in a subdirectory `pkg`.

Modules target one of three runners.

### Browser runner ([ws-web-runner](services/ws-web-runner))

Modules loaded by a web browser, or using Deno as the "web browser" in `et-ws-web-runner`.

Most are Rust built with `wasm-pack build --target web`; other languages:

- Dart
- Java
- .Net C#
- Python, using [pyodide](https://pyodide.org/) and [RustPython](https://rustpython.github.io/)
- R, using [webR](https://docs.r-wasm.org/webr/latest/) (rdata1, rcomm1) -- browser-only: webR spawns a classic
  Web Worker, which Deno's `et-ws-web-runner` does not support, so these fail there by design
- Zig, including C and C++ code

#### et-ws-web-runner on Windows

`et-ws-web-runner` embeds V8 through `deno_core` and the `v8` crate, and cannot build on the gnullvm default
target: there is no `x86_64-pc-windows-gnullvm` `librusty_v8` prebuilt and the gnullvm build path in `rusty_v8`
is still unfinished (open PRs [denoland/rusty_v8#1880](https://github.com/denoland/rusty_v8/pull/1880) and
[#1957](https://github.com/denoland/rusty_v8/pull/1957)). Two mise target envs each give it a working Windows
build: `MISE_ENV=msvc` uses rusty_v8's native prebuilt on `x86_64-pc-windows-msvc`, and `MISE_ENV=mingw` links
that same msvc prebuilt into an `x86_64-pc-windows-gnu` binary (the CRT bridging lives in
`services/ws-web-runner/mingw-shim/` and the crate's `build.rs`).

### WASI runner ([ws-wasi-runner](services/ws-wasi-runner))

Modules built as WASI Preview 2 components and run under wasmtime:

- Rust
- Python, via [componentize-py](https://github.com/bytecodealliance/componentize-py)

### PyO3 runner ([ws-pyo3-runner](services/ws-pyo3-runner))

Native CPython modules linked via [PyO3](https://pyo3.rs) -- used for workloads that need a real CPython runtime
(e.g. PyTorch inference).

## Root module

The default UX in the web-browser is also a loadable module located in
[services/ws-server/static](services/ws-server/static).

A custom UX module can be used by setting the `ws-server` environment variable `MODULES_ROOT`.

## Protocol & API specs

The WebSocket protocol and the ws-server REST surface are described by machine-readable specs regenerated from their
Rust sources of truth by `mise run gen-specs-all`:

- **WebSocket** (AsyncAPI 3.0): [`generated/specs/ws.yaml`](generated/specs/ws.yaml).
  Source: `ClientMessage` + `ServerMessage` in `libs/edge-toolkit/src/ws.rs`. Generated clients:
  [`generated/dart-ws/`](generated/dart-ws/), [`generated/python-ws/`](generated/python-ws/), and the
  `et:ws-messages` WIT under `generated/specs/wit/deps/`.
- **REST** (OpenAPI 3.0): [`generated/specs/rest.yaml`](generated/specs/rest.yaml).
  Source: `#[utoipa::path]` annotations on the handlers in `services/{ws-server,modules,storage}`. Typed Rust
  client at [`generated/rust-rest/`](generated/rust-rest/) (consumed by `et-ws-wasi-runner` and the browser
  `data1` module).

See [`generated/README.md`](generated/README.md) for a full catalogue of what's regenerated vs. hand-maintained
under `generated/`.

## et-cli

Run an example demo scenario using et-cli

```bash
cargo install --path utilities/cli --force
et-cli generate-deployment \
  --input-file verification/local/input/facility-security-scenario.yaml \
  --output-dir verification/local/output/facility-security-scenario
```

This will generate a `mise.toml` file under `verification/local/output/facility-security-scenario`. Run the following
command to start the demo scenario:

```bash
mise run generated-scenario
```

To generate a Docker Compose deployment instead, pass `--output-type docker-compose` or set
`deployment_type: docker-compose` in the scenario input YAML. This writes `compose.yaml` to the output directory:

```bash
et-cli generate-deployment \
  --input-file verification/local/input/facility-security-scenario.yaml \
  --output-dir verification/local/output/facility-security-scenario \
  --output-type docker-compose
cd verification/local/output/facility-security-scenario
docker compose up --build
```

The generated scenario config only selects which prebuilt modules `ws-server` serves. Module builds are expected
to be handled separately from the repository root.

To regenerate all checked-in verification outputs from `verification/*/input`, writing each scenario to the
matching `verification/*/output/<input-file-stem>` folder. This generates all supported deployment files for each
scenario, currently `mise.toml` and `compose.yaml`:

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
