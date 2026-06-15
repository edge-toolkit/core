# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Scratch work stays inside this repo

Any throwaway file the agent needs while working — backup copies of files
before destructive edits, generated probe scripts, captured tool output,
intermediate diff materials, anything — must live under this repo's
`target/` directory (which is already gitignored). Do **not** write to
`/tmp`, `/var/tmp`, `~/Desktop`, `~/Downloads`, `~/scratch`, or any other
path outside this working directory. `target/scratch/` is fine; create
subdirectories under it freely and clean up when done.

## Keep lines ≤ 120 characters

`editorconfig-checker` (`ec`, wired into `mise run check` via the
`editorconfig-check` task) enforces a 120-char limit on every file the
`[*]` rule in `.editorconfig` covers — which is almost all of them. A
small number of files have explicit overrides (`LICENSE-*` and generated trees
under `generated/`), but assume every file you touch is bound
by 120 unless you have specific evidence otherwise.

When writing comments, doc strings, `#[expect(reason = "…")]` reasons,
`description = "…"` fields in TOML, markdown tables, JSON schemas, etc.,
keep each line under the limit on the first draft rather than relying
on a follow-up fix-up pass. The most common offenders are: long
`reason = "…"` strings on lint attributes, JSON `description` fields,
markdown table rows, and CI-task `description` fields.

## Document each thing exactly once

Document each thing **once**, in the single place it is most relevant —
the code, config entry, or task it describes — and **nowhere else**. Do
not restate the same explanation in a second comment, in the README, in
this file, or in a commit message that competes with it.

Do not even add a pointer to where something is documented ("see X",
"as described in Y", "(documented in Z)"). Such cross-references are
themselves duplication: they go stale, and they multiply the places that
must change when the thing changes. Trust the developer to find the
relevant documentation themselves — they know how to read the code,
`grep`, and follow the obvious file.

When you find yourself about to explain something that is already
explained elsewhere, stop: either the existing spot is the right home (so
say nothing here), or this is the better home (so move it here and remove
the original). One canonical location, never two.

## Prerequisites

Install [`mise`](https://mise.jdx.dev/) with shell integration, then configure:

```bash
mise settings experimental=true
mise settings set cargo.binstall true
mise install            # Rust + Node + universal tooling (always loaded)
```

The config is split under `.mise/`: `.mise/config.toml` is always loaded
(Rust/Node toolchain, universal linters, orchestration); per-language
`.mise/config.<lang>.toml` files (dart, dotnet, java, python, zig) are
selected via `MISE_ENV`. Install a guest language's toolchain with
`MISE_ENV=dart mise install`, or every language with `mise run install-all`.
`MISE_ENV` is comma-separated (`MISE_ENV=python,zig`) and can be exported to
make a selection sticky. Rust currently lives in the always-loaded config
(`.mise/config.rust.toml` is an empty placeholder for a later migration).

## Common Commands

All tasks run through `mise run <task>`. The aggregates below act on Rust +
the universal checks **plus whichever guest languages `MISE_ENV` has loaded**;
use the `*-all` variants (`check-all`, `test-all`, `build-modules-all`,
`gen-specs-all`) — or set `MISE_ENV` — to cover every language.

| Task                            | Command                                |
| ------------------------------- | -------------------------------------- |
| Format all (every language)     | `mise run fmt-all`                     |
| Check all (every language)      | `mise run check-all`                   |
| Run all tests (every language)  | `mise run test-all`                    |
| Build all WASM modules          | `mise run build-modules-all`           |
| Regenerate all specs            | `mise run gen-specs-all`               |
| Run Rust tests only             | `mise run cargo-test`                  |
| Run Python tests only           | `MISE_ENV=python mise run test:python` |
| Run WebSocket server            | `mise run ws-server`                   |
| Start OpenObserve (Docker)      | `mise run o2`                          |
| Download ONNX models            | `mise run download-models`             |
| E2E tests (Chrome)              | `mise run ws-e2e-chrome`               |
| Regenerate verification outputs | `mise run regen-verification`          |

**Rust formatting uses nightly:** `cargo +nightly fmt`

**Build a single module:** `MISE_ENV=<lang> mise run build-ws-<module>-module`
(e.g., `mise run build-ws-face-detection-module` for the Rust modules, or
`MISE_ENV=zig mise run build-ws-zig-data1-module`).

## Formatters & checks by file type

The `mise run <task>` formatters and checks per file type (`fmt` / `check` run
them all; guest rows need their `MISE_ENV` loaded).

| File type | Formatter task(s)               |
| --------- | ------------------------------- |
| `*.rs`    | `cargo-fmt`, `cargo-clippy-fix` |
| `*.toml`  | `taplo-fmt`                     |
| `*.py`    | `ruff-fmt`                      |
| `*.dart`  | `fmt:dart`                      |
| `*.zig`   | `fmt:zig`                       |
| `*.c`     | `clang-format`                  |
| `*.cs`    | `fmt:dotnet`                    |

| File type | Check task(s)                                                                            |
| --------- | ---------------------------------------------------------------------------------------- |
| `*.rs`    | `cargo-check`, `cargo-clippy`, `cargo-fmt-check`, `cargo-doc-check`, `ast-grep-check`    |
| `*.toml`  | `taplo-check`, `conftest-check-toml`, `semgrep-check`                                    |
| `*.yaml`  | `ast-grep-check`, `conftest-check-yaml`, `ryl-check`, `action-validator`, `zizmor-check` |
| `*.json`  | `semgrep-check`                                                                          |
| `*.py`    | `check:python`                                                                           |
| `*.dart`  | `check:dart`                                                                             |
| `*.zig`   | `check:zig`                                                                              |
| `*.c`     | `clang-format-check`, `clang-tidy-check`, `cpplint-check`                                |
| `*.cs`    | `check:dotnet`                                                                           |
| `*.java`  | `check:java`                                                                             |

`dprint-fmt` / `dprint-check` cover `*.md`, `*.yaml`, `*.json`/`*.jsonc`,
`*.ts`/`*.js`, `*.css`, `*.html`, `*.java`, and `Dockerfile*`; `hadolint-check`
also lints Dockerfiles, and `link-check` scans `*.md` + `*.rs`. Every file is
covered by `editorconfig-check` and `typos`, file and directory names by
`ls-lint-check`, and `*.yml` is rejected by `semgrep-check` (use `*.yaml`).

## Architecture

This is a WebSocket-based edge computing framework.

The server is a hub: it maintains an agent registry, routes messages between agents, provides agents with storage,
and serves node module packages as static files to browsers.

### WebSocket protocol

The Rust sources of truth are `ClientMessage` (what clients send) and
`ServerMessage` (what the server sends) in `libs/edge-toolkit/src/ws.rs`.
The full message catalogue (every wire `type`, request/response shape, and shared schema) is
regenerated by `mise run gen:ws-spec` into [`generated/specs/ws.yaml`](generated/specs/ws.yaml)
(AsyncAPI 3.0). Generated language clients sit next to it under `generated/dart-ws/`,
`generated/python-ws/`, and `generated/specs/wit/deps/et-ws-messages/`.

The server acts as a pure hub for **unrecognised** frames: any text the server can't parse as a
known `ClientMessage`, and any binary frame, is forwarded verbatim to every other connected agent
(with a single `info!` log per broadcast). This lets agents use arbitrary out-of-band payloads
without needing a server-side enum entry. Explicit `et-broadcast-message` still wraps payloads in
an `et-agent-message` envelope as before. Both paths require the sender to be a connected agent;
frames from unassigned clients are dropped.

### REST surface

Every HTTP endpoint exposed by ws-server (health probe, module discovery, module assets,
per-agent storage) is annotated with `#[utoipa::path]` in its handler. `mise run gen:ws-spec`
emits the aggregated [`generated/specs/rest.yaml`](generated/specs/rest.yaml) (OpenAPI 3.0)
and the typed Rust client at [`generated/rust-rest/`](generated/rust-rest/) via `progenitor`
— consumed by `et-ws-wasi-runner` (native) and the browser `data1` module (WASM). The client
crate's `tracing` feature is on by default (W3C `traceparent` injected on every request via a
progenitor pre-hook) and off in WASM consumers, which switch reqwest to its `fetch()` transport.

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
- **Python (componentize-py → WASI Preview 2 component)**: wasi-graphics-info — runs in
  `et-ws-wasi-runner` rather than the browser. The WIT world the component implements is at
  `services/ws-wasi-runner/wit/world.wit` and is mirrored under the module's own `wit/`.
  Drives two standardised WASI interfaces end-to-end: (1) `wasi:webgpu/webgpu` (trimmed subset
  of WebAssembly/wasi-gfx) for a real 4x4 compute matmul through a host wgpu device, and
  (2) `wasi:nn/{graph, tensor, inference}` for MNIST inference. Bundles `mnist-12.onnx` (served
  from `pkg/` as a static asset; the guest fetches it via the `storage` host import because
  componentize-py 0.23 doesn't bundle non-Python data files), then runs inference through ONNX
  Runtime via `wasi:nn/graph.load` + `inference.compute` and verifies the predicted class.

### Libraries (`libs/`)

- **edge-toolkit** — Common utilities, config, serialization (shared across services)
- **web** — WASM web helpers (Canvas, MediaStream, WebSocket bindings for browser modules)

### WASI Runner (`services/ws-wasi-runner/`)

`et-ws-wasi-runner` — runs ws-modules compiled to **WASI Preview 2 components** (rather than
browser WASM modules). It fetches the module's `pkg/package.json` from the ws-server, downloads
the `.wasm` named by the `wasi-main` field, instantiates it under `wasmtime` with async support,
and calls the exported `entry.run` function.

Host imports (defined in `wit/world.wit`, package `et:ws-wasi@0.1.0`):

- `log` — `log` and `set-status` for guest output
- `clock` — `sleep-ms`, `now-ms`
- `storage` — `put-file`/`get-file` proxied to the ws-server's storage service via reqwest
- `ws` — websocket client backed by `tokio-tungstenite`; mirrors the wire format of
  `et-ws-wasm-agent` so events look the same on the server

Plus, attached to the same Linker but defined by external WIT packages:

- `wasi:webgpu/webgpu@0.0.1` — trimmed subset of WebAssembly/wasi-gfx, vendored under
  `wit/deps/wasi-webgpu/`. Compute-only (render pipelines, textures, samplers, canvas/surface,
  query sets, async pipeline creation are stripped; the trimmed surface is just what's needed
  to run a compute pipeline through to a mappable readback buffer). The host impl in
  `src/host/wasi_webgpu.rs` is wgpu-backed (Metal / Vulkan / DX12) for the matmul path;
  every other kept method traps with `unimplemented!`. We carry this divergence from upstream
  because wasi-gfx isn't published to crates.io — replace this whole tree with the upstream
  WIT plus its matching host crate once it ships.
- `wasi:nn/{tensor, graph, inference, errors}` — standardised ML inference. The host wires
  `wasmtime-wasi-nn` with the ONNX Runtime backend (`ort` 2.0.0-rc.10, pinned because rc.11+
  moved API surface that wasmtime-wasi-nn 44 still uses). Guests load model bytes via
  `graph.load`, build `Tensor`s, and call `compute` — the same shape of calls Spin / wasmCloud
  / Fermyon production wasi-nn workloads use. CUDA dispatch is opt-in via the runner's
  `cuda` cargo feature (`cargo build -p et-ws-wasi-runner --features cuda` or
  `RUNNER_FEATURES=cuda mise run ws-wasi-runner`); the default build is CPU-only because
  Pyke's `ort` download-binaries CUDA prebuilt only exists for some triples (notably
  Linux x86_64). CoreML-on-macOS would need a wasmtime-wasi-nn patch (its `onnx.rs` only
  knows the CUDA provider for `ExecutionTarget::Gpu`).

`RUNNER_MODULE` selects the module (e.g. `wasi-graphics-info`). `WS_SERVER_URL` defaults to
`ws://localhost:8080/ws`.

### Utilities (`utilities/`)

- **cli** (`et-cli`) — Scenario and module tooling. Reads scenario YAML and outputs `mise.toml` or `compose.yaml`;
  also generates module `pkg/package.json` files with `et-cli module-package-json`.
  Deployment-specific generators live under `utilities/cli/src/deployment_types/`.
  Module package JSON generation lives under `utilities/cli/src/module_package_json/`.
- **onnx** — ONNX model utilities

### Verification (`verification/`)

Scenario YAML inputs live in `verification/*/input/`. Expected outputs are checked into `verification/*/output/`
and must stay in sync — `mise run check` will fail if they drift. Regenerate with `mise run regen-verification`.

## Module Build Details

- Most Rust modules: `wasm-pack build . --target web` from the module directory
- WASM agent (nightly, MVP target): uses `RUSTFLAGS="-C target-cpu=mvp ..."` and `RUSTUP_TOOLCHAIN=nightly`
- `har1` and `face-detection`: after wasm-pack, merge extra `package.json` fields with `yq`
- Python modules: `uv build --wheel` then `cargo run -p et-cli -- module-package-json`
- WASI Python modules (`wasi-graphics-info`): `componentize-py -d wit -w module bindings .` then
  `componentize-py -d wit -w module componentize <pkg> -o pkg/<pkg>.wasm` then
  `cargo run -p et-cli -- module-package-json`. The `[tool.ws-module] wasi-main` field flows to
  `package.json` so `et-ws-wasi-runner` knows which file to fetch.
- Rust modules needing dependency injection: `cargo run -p et-cli -- module-package-json`
  merges `[package.metadata.ws-module.dependencies]` from `Cargo.toml` into `pkg/package.json`
- `et-cli module-package-json` reads `pyproject.toml` (Python modules, via `[tool.ws-module]`)
  or `Cargo.toml` (Rust modules, via `[package.metadata.ws-module]`).
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

Every file under `tests/` must start with `#![cfg(test)]` (placed after the file's `//!` doc comment, if any).

## Tools must work on every OS

Every tool in the `.mise/config*.toml` `[tools]` tables must install and run on
every supported OS (Linux, macOS, Windows). Do **not** `os`-scope a tool, or
otherwise skip it on a platform, without explicit operator permission — prefer a
prebuilt-binary backend (aqua/github/http) over a `cargo:` source build, which is
usually what forces a platform exclusion. The one place tool skips need no
permission is the Dockerfiles (`MISE_DISABLE_TOOLS`), where trimming an image to
just what its build needs is expected.

## Linting

Lint checks must be expressed through one of the repo's linters — **never** as a
bespoke shell script, whether a standalone file or a mise task `run`. The
available linters:

- **ast-grep** (`config/ast-grep/rules/`) — structural rules for code **and
  YAML** (e.g. GitHub Actions workflows).
- **semgrep** (`config/semgrep/`) — incl. `languages: [generic]`, which works on
  TOML/text (e.g. `mise-config.yaml` lints `.mise/config*.toml`).
- **taplo** JSON-schemas (`config/taplo/`) — TOML structure, applied via
  `taplo lint --schema` in `taplo-check`.
- **conftest** (`config/conftest/policy/`) — Rego policies over the combined
  TOML/YAML config set, for cross-file checks the schema linters can't express.
- plus hadolint, ls-lint (file/dir naming), zizmor (Actions security), ryl
  (YAML), lychee (links), clang-format / clang-tidy / cpplint (C, in the zig
  config), editorconfig-checker, typos, and action-validator for their domains.

ast-grep has no TOML grammar, so it **cannot** lint TOML — use a taplo schema or
a semgrep `generic` rule there. If none of the above can express a check,
propose adding a new mise-installable linter rather than scripting it by hand.

## No `scripts/` directory

Do not create a `scripts/` directory or drop loose shell/Python scripts in the
repo. Every script belongs in one of two places:

- **Short and simple** → an inline `mise` task (`run = """ … """` in
  `.mise/config.toml` or a `.mise/config.<lang>.toml`). It stays discoverable
  via `mise tasks` and runs as `mise run <name>`.
- **More involved** → its own tool directory under `utilities/` with its own
  `README.md` documenting what it does and how to run it.

## Don't depend on host tools in mise tasks

A mise task must never assume a command-line utility happens to exist on the host
(or in a base image). Use the mise-managed, version-pinned, cross-platform tool
instead, so a task behaves identically on CI, in the Docker images, on a
workstation, and on every OS. Reach for a host binary only if there is genuinely
no mise tool for it — and then add one rather than depending on the host.

What to use instead of the common host utilities (a list, not a table — dprint
pads table columns, which blows the 120-char limit):

- `cut`, `ls`, `sort`, `mktemp`, `cat`, … → `coreutils <util>` (uutils multicall;
  always invoke with the explicit `coreutils` prefix)
- `grep` → `rg` (ripgrep)
- `find`, `xargs` → bare `find` / `xargs` (uutils `findutils` mise tool; its shims
  shadow the host's)
- `awk` → `goawk`
- `sed` → no tool; rewrite the step with `coreutils`, `rg -r`, or `goawk`

`coreutils`, `ripgrep`, `findutils` and `goawk` are mise `[tools]`. `coreutils`
and `ripgrep` are additionally force-installed by `_setup_all`, because the
`preinstall` task itself uses them before the main `mise install` runs.

What the Dockerfiles `apt-get install` is therefore only genuine build
prerequisites the toolchain needs (compilers, libraries, the archive tools mise
unpacks downloads with) — never POSIX utilities, which now all come from tools.

One Nano Server exception: `Dockerfile.nanoserver` does not put mise's shims on
`PATH` (native busybox-w32 can't use the msys-form paths mise injects for POSIX
shells — see the `http:busybox` note in `config.windows.toml`), so the Windows
`preinstall` can't call these tools bare — it goes through `mise exec --` or a
shell builtin instead. **TODO (next time we improve `Dockerfile.nanoserver`):**
work out a busybox-compatible way to get the shims (or tool bins) onto `PATH` so
Windows tasks can call `coreutils`/`rg`/`goawk` directly like every other OS, and
drop the `mise exec --` / shell-builtin workarounds.

## Rust Workspace

Single Cargo workspace (`Cargo.toml`).
Shared dependency versions are declared in `[workspace.dependencies]`.
Add new deps there, not in individual crate `[dependencies]`.

## Clippy lints

**Never weaken or disable a lint to make code pass — not the workspace lint
config (`[workspace.lints.*]`, `.clippy.toml` thresholds, the ast-grep / taplo
rules) — without explicit operator permission.** Setting a denied lint to
`allow` or raising a threshold is a project-policy change, not a fix. If a lint
is in the way, fix the code (or justify it with a scoped
`#[expect(..., reason = "…")]`); if you believe the lint itself is wrong, stop
and ask.

Clearing an `#[expect(...)]` that clippy reports as **unfulfilled** is normally
fine and expected — that's the intended cleanup. **The one exception is
`clippy::cognitive_complexity`** (and other macro-expansion-sensitive lints):
do **not** auto-remove those `#[expect]`s, because of the gotcha below.

**Feature-unification gotcha.** `clippy::cognitive_complexity` can fire in the
full-workspace build but not in an isolated `cargo clippy -p <crate>`, because
`-p` enables fewer features (e.g. the `tracing` macros expand further when the
whole workspace's tracing/otel features are on). So its `#[expect]` can look
_unfulfilled_ in an isolated run yet be _required_ by CI. **Validate with the
full `mise run check`, not an isolated `-p` clippy, before touching one of these
`#[expect]`s.** (The clean fix — `[resolver] feature-unification = "workspace"`
so every build sees the same features — is still nightly-only via
`-Z feature-unification`.)

The workspace denies a broad set of clippy lints (see `[workspace.lints.clippy]`
in `Cargo.toml`), including restriction lints. One you'll hit often:
**`clippy::single_call_fn`** fires on a private function called from exactly one
site. Do **not** inline the function just to silence it — a function that is a
distinct, named step (kept separate for readability, or that will gain more
callers) is legitimate. Keep it and annotate with `#[expect(...)]` and a real
justification:

```rust
#[expect(clippy::single_call_fn, reason = "distinct step of X; kept separate for readability and future reuse")]
fn helper(...) { ... }
```

Use `#[expect(...)]` rather than `#[allow(...)]` (the workspace denies
`unfulfilled_lint_expectations`, so an `expect` that stops applying fails the
build instead of silently lingering). The same applies to other restriction
lints whose pattern is intentional in a given spot — prefer a justified
`#[expect(..., reason = "…")]` over contorting the code to dodge the lint.

## Naming conventions

- **`.map_err` wrappers must be named `map_*`.** Extension methods that
  hide a `.map_err(...)` call (e.g. converting a foreign error to a
  domain error type) must keep `map_` in the name. The reader can then
  tell at the call site that this is a _mapping_ over the error, not
  some unrelated boolean predicate. Example: the `JsErrExt` trait in
  `services/ws-web-runner/src/error.rs` exposes `.map_js_err()` and
  `.map_js_err_with_context(...)`, never `.js_err()` / `.to_js_err()` /
  similar.
