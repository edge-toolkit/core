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

## On macOS: use homebrew bash for ad-hoc agent commands

On macOS, every ad-hoc command the agent runs that is **not** a `mise run
<task>` invocation — investigations, scratch-area probes, multi-line
shell loops, anything with command substitution or arrays — must execute
under homebrew's `bash` (typically `/usr/local/bin/bash` on Intel,
`/opt/homebrew/bin/bash` on Apple Silicon), **not** the session's default
zsh and **not** macOS's bundled `/bin/bash` (which is GNU bash 3.2 from
2007 and predates many modern features).

Invoke it explicitly per command: `/usr/local/bin/bash -c '<script>'` (or
the Apple-Silicon path). Mise tasks are exempt — they run under their
own configured shell (`bash -euo pipefail -c` etc.) which is already
correct.

Reason: agents are bad at zsh-specific quoting and expansion rules, and
the system bash is too old to behave like the bash everyone else uses.
Pinning to homebrew bash makes ad-hoc shell behavior predictable and
matches what mise tasks already use.

## Polling cadence when monitoring a PR's CI runs

When an agent is watching a PR (e.g. via `/loop`), the next-poll delay
follows three tiers — tight at first (catch fail-fast errors), loose in
the middle (long compile/test phases run on a 30-90 min scale), then
loose again once everything's settled (waiting for the user to push):

- **First 5 minutes after a push**: poll every **1 minute**. Most
  startup / preinstall / fail-fast errors surface in this window.
- **Minute 5 to 1 hour**: poll every **5 minutes**. The long phases
  (cargo builds, docker stage builds, full test matrix) finish on this
  scale; sub-5-min polling here burns the prompt cache without
  catching anything sooner.
- **After 1 hour, OR once all jobs have stopped**: poll every
  **20 minutes**. The polling now is mostly waiting for the next user
  push — at this cadence the prompt cache misses anyway, so spending
  it sparingly on a heartbeat is the right trade.
- **Stop 1 hour after all jobs have stopped.** If the user hasn't
  re-pushed by then, they will tell the agent to restart monitoring.
  Stopping is implemented by omitting the next `ScheduleWakeup` call
  (see the `/loop` skill's "To stop the loop" note).

`/loop` dynamic-mode wakeups are bounded [60, 3600] by the runtime, so
each cadence maps directly: 1 min → `delaySeconds: 60`, 5 min →
`delaySeconds: 300`, 20 min → `delaySeconds: 1200`.

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

## No trailing-backslash line continuations

Trailing-backslash line continuations (a line ending in `\` to join with
the next) are **banned everywhere** in this repo. The only exceptions
are README files and generated trees (e.g. `verification/`). Character
escapes _inside_ string literals — `"\n"`, `"\t"`, regex `\d`, a
Windows path `C:\Foo\Bar` as a value — are NOT continuations and are
fine; the ban is on the end-of-line `\` that joins source lines into
one logical statement.

If a logical line would otherwise exceed the 120-char limit, pick a
no-backslash split. Some patterns that work in the languages this repo
uses:

- **mise task `run` bodies**: factor into shell variables / `[vars]`
  entries / multiple statements. Do this on the first draft (see also
  the line-limit rule above), not as a fix-up pass.
- **Dockerfile ENV / ARG**: build the value across multiple `ARG`s and
  compose the final `ENV` from them via `${VAR}` expansion. Example
  from `Dockerfile.nanoserver`'s `MISE_DISABLE_TOOLS`:

      ARG MISE_DT_BASE=cargo:dart-typegen,conda:m2-gnupg,...
      ARG MISE_DT_PY=pipx:componentize-py,pipx:...
      ENV MISE_DISABLE_TOOLS=${MISE_DT_BASE},${MISE_DT_PY}

- **Dockerfile `RUN` block**: switch to BuildKit's HEREDOC form
  (`RUN bash <<'EOF'` … `EOF`) — each shell command sits on its own line
  with no continuation needed. Three rules, all enforced by
  `config/conftest/policy/dockerfile.rego` + the matching semgrep rule
  under `config/semgrep/`: (1) interpreter is **`bash`, placed BEFORE the
  `<<TAG`** (default `/bin/sh` on Debian/Ubuntu is dash, which rejects
  `set -euo pipefail`; the inverse `RUN <<EOF bash` form is silently
  broken because BuildKit treats trailing tokens as literal); (2) the
  **delimiter must be quoted (`<<'EOF'`)** — with an unquoted `<<EOF`,
  the outer `/bin/sh -c` that wraps the RUN performs `$(...)` command
  substitution on the body BEFORE bash runs, so `libicu=$(apt-cache ...)`
  is evaluated against the outer shell at the wrong moment (Fedora
  aborted with `apt-cache: command not found`; Debian/Ubuntu ran
  apt-cache before the script's `apt-get update`, getting a stale
  cache); quoting defers every expansion to bash, and ARG values needed
  inside the body must be promoted to ENV beforehand (`ARG FOO=...` →
  `ENV FOO=${FOO}`); (3) **first body line must be `set -euo pipefail`**
  — HEREDOC RUNs go through bash without inheriting strict mode from any
  outer setting, so the invariant has to be re-declared inside every
  body. Leave a blank line between the closing `EOF` and the next
  instruction — hadolint's parser otherwise errors with `unexpected 'E'
  expecting a new line...`.
- **YAML run-bodies**: a `|` block scalar already keeps each shell
  line natural; no continuations are necessary.
- **Long flag lists**: drop them into a config file (`.env`, `.cfg`,
  task `[vars]`) or split into one flag per line inside a HEREDOC /
  block scalar.

If a split honestly isn't possible without `\`, stop and surface the
constraint — do not reintroduce the regression. Existing `\` uses in
the repo (Dockerfile RUN-block multi-liners, etc.) are regressions
slated for cleanup, not precedent.

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

**Agents must not run `mise run check` (or `check-all`).** They are aggregate
gates intended for CI and humans — running the whole battery for every edit
is slow, expensive, and almost always wasteful when only a couple of file
types changed. Pick the targeted tasks from the tables below that match the
extensions of the files you actually modified, and run only those. Same goes
for `fmt`: run the per-file-type formatter, not the aggregate.

**Agents must not invoke formatter/linter binaries directly** (e.g.
`mise exec -- taplo format`, raw `taplo`/`dprint`/`oxfmt`/`cargo fmt` calls).
Always use the corresponding mise task (`mise run taplo-fmt`, `mise run
taplo-check`, `mise run dprint-fmt`, etc.). The tasks carry the project's
config-file paths (`config/taplo.toml`, `config/dprint.jsonc`, …), exclusion
lists, and flag conventions; bypassing them produces results that don't match
the canonical pipeline (different excludes, different `reorder_keys`/
`column_width` settings, different file globs). taplo specifically: the only
right way to format TOML in this repo is `mise run taplo-fmt`.

The `mise run <task>` formatters and checks per file type. The aggregates
(`fmt`/`check`/`fmt-all`/`check-all`) run every loaded language's row; guest
rows need their `MISE_ENV` loaded.

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

For Rust inner-loop iteration on a single crate, use `mise run
cargo-clippy-check-pkg <package>` (alias `clippy-pkg`) instead of
`cargo-clippy-check` — it runs `cargo clippy --keep-going --tests -p
<package>` so you only compile that crate plus its deps, not the whole
workspace. Same lint config; just narrower scope. Switch back to
`cargo-clippy-check` for the final verification pass before declaring
done, since cross-crate `feature-unification` differences can fire
workspace-wide lints that don't fire on a single `-p` build.

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
- **int-gen** (`et-int-gen`) — Internal code generator emitting artifacts under `generated/` from in-repo Rust
  sources of truth (AsyncAPI/OpenAPI YAML, WIT, KDL, schema JSON, the typed Rust REST client, the Zig client).
- **onnx** (`et-onnx`) — ONNX model utilities.

Each utility has a committed `HELP.md` (under its crate dir) that mirrors the clap-derive tree via the
`markdown-help` feature + hidden `--markdown-help` flag. **Read `utilities/<name>/HELP.md` to learn what
the CLI does — don't run `cargo run -p <name> -- --help`** (much slower: cargo has to build the binary
first; HELP.md is the same content as a static file). `mise run gen-help-all` regenerates them all; the
`gen-help-check` task (wired into `check:rust`) fails on drift.

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

## Workarounds

When you can't (or shouldn't) fix the root cause right now — a libc race in
an upstream dep, a flaky platform driver, a runner-image quirk, a toolchain
bug, a CI-only flake — and you decide to paper over it with a workaround:

- **Gate narrowly to the affected situation.** Use `#[cfg(target_os = "…")]`
  / `target_arch` / `target_env` (or the equivalent in YAML / Dockerfiles /
  build scripts) so other platforms keep exercising the real path. Don't
  blanket-disable.
- **Gate to only the use site that needs it.** For test-only quirks, a
  test-set env var the production code checks works well. For build-time
  quirks, a feature flag or build profile. The default behaviour on every
  platform should stay the unworkarounded one wherever it works.
- **Embed the exact error message verbatim** at the workaround site — a
  comment block quoting the upstream panic / abort / linker error / test
  failure line. GHA log retention is 3 months; once the run is gone, the
  only way someone hitting the same symptom later finds your workaround is
  by grepping the repo for the error string. This applies equally to
  symptoms first seen locally and to GHA-only flakes. Alongside the error
  string, **record the commit SHA the failure was observed on** (full
  40-char hash, so the comment stays unambiguous after force-pushes /
  rebases) and, when applicable, **the GHA job-run URL**
  (`https://github.com/<owner>/<repo>/actions/runs/<run-id>/job/<job-id>`).
  Both age out (the SHA may stop existing if a branch is deleted; the GHA
  log expires at 3 months) but together they pin the WHERE and WHEN of the
  evidence well enough for the next reader to cross-reference your local
  notes, screenshots, or any persisted artifact.

## Non-negotiable platform constraints

Project decisions pinned here are not subject to "easier path" rewrites,
even when something downstream is in the way. Don't propose disabling or
working around them; find a compliant solution instead.

1. **`Dockerfile.nanoserver`'s base image stays Nano Server.** Don't switch
   to Windows Server Core / LTSC / any non-Nano-Server base, no matter how
   much it would unblock a tool. Minimal image size on the Windows lane is
   load-bearing.
2. **`[settings] gpg_verify = true` stays in `.mise/config.toml`.** The
   cross-platform default is "verify". The one allowed scope-down is
   `ENV MISE_GPG_VERIFY=false` inside `Dockerfile.nanoserver` (only that
   file), and only while the mise + Nano `gpg --import` / `gpg --verify`
   pipe behavior is broken upstream. The matching gpg binary stays
   installed (see the `gpgbin` donor stage) so flipping the env back to
   `true` is a one-line revert once mise stops panicking on Nano. Every
   other platform still hard-fails when gpg is unreachable.

Recorded here so future iterations don't re-litigate the same trade-off.

## Tools must work on every OS

Five supported platforms in two tiers. **Main tier:** macOS arm64, Linux x64,
Windows x64 — every tool in the `.mise/config*.toml` `[tools]` tables must use a
prebuilt-binary backend (aqua/github/http) here; a `cargo:` source-build isn't
acceptable. **Second tier:** Linux arm64, macOS x64 — every tool must still
install and run, but slower install mechanisms are allowed because release
authors often skip prebuilts for these arches: a `cargo:` source-build (or
alternate backend) `os`-scoped to a second-tier-only platform is fine. The
conftest mise policy (`config/conftest/policy/mise.rego`) enforces both rules,
including the narrow per-name allowlist for tools that have no prebuilt at
any triple.

Skipping a tool entirely with `MISE_DISABLE_TOOLS` is reserved for
`Dockerfile.nanoserver`, where trimming the image to just what its build needs
is expected. Other Dockerfiles may only disable tools that are unused by the
build system anyway (e.g. `cargo-expand`, a dev-only macro-debugging tool).

## Adding a new `upstream-cache` entry

When upstream has no prebuilt binary for a platform we target (the case
the "Tools must work on every OS" rule needs help solving), we mirror
the build to our own GitHub release and point `[tools."http:<name>"]`
at it. The pattern is the same for every cache entry; copy the
`rustpython` / `augeas` / `dart-typegen` / `gnupg-w32` shape:

1. **Bootstrap task in `.mise/config.maint.toml`.** An idempotent
   `[tasks.bootstrap-<name>-release]` that runs `gh release view <tag>
   ... || gh release create <tag> ... --prerelease`. The `--notes`
   field must contain **only the upstream project URL and its SPDX
   license expression, separated by a newline — nothing else** (no
   prose, no "auto-built by..." footer). The maintainer runs this
   once per repo before the workflow's first dispatch.

2. **Job in `.github/workflows/upstream-cache.yaml`.** Mirrors the
   rustpython job's shape: a `Detect missing <name> asset` step that
   queries the release via `gh release view --json assets`, gates the
   build/upload steps on `outputs.work == 'yes'`, then a build step,
   then a publish step that runs `gh release upload <tag> --clobber`.
   The job creates **no** release — the bootstrap task does. The job
   uploads only.

   **Use pkgx's pantry recipe as the canonical source of build
   instructions.** Before writing the build step, fetch
   `https://github.com/pkgxdev/pantry/blob/main/projects/<upstream>/package.yml`
   (or the closest match — pkgx organizes by upstream domain, e.g.
   `augeas.net`, `gnupg.org`) and lift its `build.script`,
   `build.dependencies`, and `build.env` verbatim where possible. The
   pantry encodes years of accumulated knowledge about quirks (gcc-14
   `-Wno-implicit-function-declaration`, `--disable-debug`, autoreconf
   ordering) that we'd otherwise rediscover the hard way. Where mise
   has a matching `pkgx:<name>` backend entry for a build-time tool
   pkgx pulls in, prefer that over an MSYS2/conda/apt equivalent so
   the same toolchain version installs on every developer machine.

3. **Tarball layout.** Flat `bin/`+`lib/`+`share/`+`include/` rooted
   at the tar prefix (no nested `install/` or `<pkg>-<ver>/` segment).
   For Windows binaries, bundle dependent DLLs into `bin/` so the
   tarball is self-contained on a vanilla host (no MSYS2/conda needed
   at consumer time). The asset filename should embed the version + the
   target triple (e.g. `augeas-1.14.1-x86_64-pc-windows-mingw.tar.gz`)
   so multi-platform releases don't collide.

4. **`http:` tool entry.** Add `[tools."http:<name>"]` in
   `.mise/config.toml` (cross-platform tools) or
   `.mise/config.windows.toml` (Windows-only). Required fields per
   platform: `url`, `checksum = "sha256:..."`, `version`. The publish
   step should emit a `.sha256` sidecar alongside the tarball so the
   maintainer can paste the value into config.toml. (The rustpython
   publish task auto-edits config.toml via `cargo:toml-cli` when
   available — same pattern is fine for new entries.)

5. **Asset metadata source of truth: `config/upstream-cache/data.toml`.**
   Every tarball / wheel / model file fetched from one of our releases
   (or from any upstream URL) gets an `[asset."<filename>"]` table:
   ```toml
   [asset."<asset-filename>"]
   sha256   = "<sha256-hex>"   # may be "" while bootstrapping a new entry
   url      = "<download URL>"   # canonical fetch URL (our release tag)
   upstream = "<upstream project URL>"
   license  = "<SPDX expression>"
   ```
   The matching `.mise/config*.toml` `[vars]` block defines a
   `<name>_asset` var pointing at the same filename (so the fetch
   command, the integrity check, and the metadata table share one
   source). The `config/conftest/policy/checksums/checksums.rego`
   policy enforces both the bidirectional cross-reference (every
   `<name>_asset` ↔ `[asset.<filename>]`) and the per-entry shape
   (`url` + `upstream` + `license` required; `sha256` present,
   optionally empty during bootstrap). Bump all rows in lockstep
   when the upstream version changes.

5. **Triggers.** `pull_request` on `paths: [.github/workflows/
   upstream-cache.yaml, .github/actions/install-mise/**]` so editing
   the workflow exercises the job before merge; `workflow_dispatch`
   for ad-hoc rebuilds (e.g. version bump). No `push:` trigger — we
   don't want a `main`-merge to rebuild assets.

Required side-effect: an `etc-` entry in `config/conftest/policy/
mise.rego`'s allowlist of `http:` tools that have no prebuilt at any
triple, if applicable. (Skip if the tool is OS-scoped and the
allowlist already covers it.)

## taiki-e/install-action's resolution chain

The `.github/actions/install-mise` composite action uses
[`taiki-e/install-action`](https://github.com/taiki-e/install-action) to fetch
the mise binary plus any tools passed via its `extra-tools:` input. When
deciding whether install-action can supply a given tool, walk its three-tier
chain:

1. **install-action's own TOOLS manifest** — its
   [`TOOLS.md`](https://github.com/taiki-e/install-action/blob/main/TOOLS.md)
   is the authoritative list of tools it ships hand-curated prebuilt-URL
   manifests for. Fastest path; one HTTP fetch per tool.
2. **cargo-quickinstall fallback** — if the tool name isn't in the manifest,
   install-action hands off to `cargo-binstall`, which in turn checks
   [cargo-quickinstall's release index](https://github.com/cargo-bins/cargo-quickinstall/releases)
   for prebuilt assets. This covers a much wider set of crates than the
   curated manifest (anything with enough downloads gets auto-published).
3. **Source build via `cargo install`** — when neither of the above has a
   prebuilt for the target triple, cargo-binstall falls through to a real
   source build. Slow; treat its presence in CI as a regression.

When the question is "can install-action install X?", check the TOOLS.md
first; if not there, search cargo-quickinstall's release tags for `X-<ver>`.
A hit in either tier means install-action will fetch a prebuilt; absence in
both means CI would pay a source-build cost.

Even a manifest hit isn't a guarantee — install-action's resolver is strict
about the binary names it expects to find inside the prebuilt archive, and
upstream renames break it silently. Concrete cautionary tale: `aube` IS in
install-action's manifest, but the manifest expects an `aubr` binary
(`When resolving aube bin aubr is not found. This binary is not optional
so it must be included in the archive`), which recent aube releases don't
ship. install-action then falls through to a real `cargo install` source
build, which then flakes on crates.io. The mise-managed `setup-aube` task
(npm-backed, `continue-on-error`) was the reliable path here; reach for
install-action only when the prebuilt actually exists for the target triple
_and_ the binary names still match.

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
- **regal** (`config/regal.yaml`, via `regal-check`) — lints the Rego policies
  themselves (style, idioms, bugs). Carves out conftest-specific patterns
  (multiple `deny` rules per file, cross-package `data.*` references, no OPA
  entrypoints) that vanilla regal would flag.
- **shellcheck on mise task bodies** (`.mise/shellcheck-mise.jq`, via
  `shellcheck-mise-check`) — extracts every multi-line bash-shell `run = """ …
  """` from `.mise/config*.toml`, masks Tera tokens (`{{ … }}` → `MISEVAR`,
  `{% … %}` → empty), and shellchecks the lot. This is how shell-quality
  lints reach mise task bodies (shellcheck itself doesn't read TOML).
- plus hadolint, ls-lint (file/dir naming), zizmor (Actions security), ryl
  (YAML), lychee (links), clang-format / clang-tidy / cpplint (C, in the zig
  config), editorconfig-checker, typos, and action-validator for their domains.

ast-grep has no TOML grammar, so it **cannot** lint TOML — use a taplo schema or
a semgrep `generic` rule there. If none of the above can express a check,
propose adding a new mise-installable linter rather than scripting it by hand.

### When you spot a style or consistency issue, write a rule

If a code-review comment, a fix-up commit, or a CLAUDE.md paragraph would tell
the next contributor "don't do X" or "always do Y", that's evidence the
codebase wants a _rule_, not just a note. Reach for the linter stack first:
which of the available tools (ast-grep / semgrep / taplo / conftest / regal /
shellcheck-mise / …) can express the rule? Code-as-policy stays in sync with
the codebase; prose drifts. Documentation has its place when the rule is
fundamentally judgement-based (the "Workarounds" section's "embed the exact
error message verbatim" guidance, say) — but if the check is mechanical, make
it mechanical.

### When you write a rule, try to make it auto-fixable

ast-grep rules accept a `fix:` field; semgrep rules accept `fix:` / `fix-regex:`;
clippy lints can be machine-applicable; the repo's `*-fix` mise tasks
(`ast-grep-fix`, `semgrep-fix`, `cargo-clippy-fix`, `ruff-fix`, `regal-fix`,
`typos-fix`, `oxlint-fix`, `clang-tidy-fix`, and the `fix` aggregator that
runs them all under `fix:rust` / `fix:<lang>` namespacing) apply those fixes
in place. A check that takes one mechanical rewrite to satisfy is much cheaper
to land than one that requires a human edit per site, so the autofix scales
the rule. When the rewrite can't be expressed as a single template (multiple
match shapes, context-dependent replacement, structural restructuring), keep
the rule check-only and write a brief note in the rule body explaining why
the autofix wasn't viable.

### NEVER delete a lint rule without explicit user permission

**Do not delete a rule from `config/ast-grep/rules/`, `config/semgrep/`,
`config/conftest/policy/`, `config/taplo/`, `config/regal.yaml`, the
`.editorconfig`, or any other linter ruleset without explicit operator
sign-off, even when the rule is "in the way" of a change you're making.**
Rules exist because someone hit a specific regression and codified the
prevention. Deleting one removes the guardrail without removing the failure
mode. If a rule is blocking work:

- Prefer narrowing it (e.g. adding a name-prefix carve-out, a path exclude,
  or a `not:` constraint) over deleting it.
- If the rule has a genuine duplicate elsewhere (another linter that checks
  the same thing), removing one of the pair is a project-policy change and
  still needs the user to OK it explicitly — say which rule you want to drop
  and why, and wait.
- If the rule is genuinely obsolete (the bug it prevents no longer exists in
  the codebase, the language/tool it targeted is gone, etc.), propose the
  removal and wait for confirmation.

A previously-incorrect call here: deleting `config/ast-grep/rules/gha-no-step-shell.yaml`
on the grounds that it was "redundant with the conftest rule in gha.rego." Even
though both rules checked the same thing, removing the ast-grep one took out a
diff-time safety net that catches the issue in IDE / pre-commit contexts where
conftest isn't always run. The right move was to add the same carve-out to both,
not delete one.

## Writing JS/TS that both dprint and oxfmt accept

Both formatters run on every `*.js` / `*.ts` file. `config/dprint.jsonc`'s
`typescript` block and `config/oxfmtrc.jsonc` are tuned to agree on the
structural choices each tool exposes as config knobs (arrow-paren style,
binary-operator position, member-chain breaking, line width). Two
unconfigurable structural decisions still trip them up — both reduce to:

> **When an assignment statement exceeds `printWidth` (120), dprint and oxfmt
> break it in different places.**

dprint inserts a linebreak after `=`, keeping the RHS one connected unit.
oxfmt prefers to keep the assignment one-line and break inside the RHS, OR
in template-literal cases, refuses to break the literal at all and overflows
silently. Either way they disagree, and there's no dprint/oxfmt knob to
reconcile them.

The fix is in the source, not the config: **keep every assignment statement
under 120 chars**. When the RHS is genuinely long, extract intermediates.

The `<!-- dprint-ignore -->` lines below stop dprint reformatting the BAD
examples (which would otherwise hide the bug we're showing).

<!-- dprint-ignore -->
```js
// Bad: long template literal assignment -- dprint breaks after `=`, oxfmt
// keeps inline, neither check is happy after the other has run.
logEl.textContent = `Initializing WASM from ${someLongUrl}\nWebSocket endpoint: ${wsUrl}`;
```

```js
// Good: extract the long sub-expression first.
const wasmUrl = "/modules/et-ws-wasm-agent/et_ws_wasm_agent_bg.wasm";
logEl.textContent = `Initializing WASM from ${wasmUrl}\nWebSocket endpoint: ${wsUrl}`;
```

<!-- dprint-ignore -->
```js
// Bad: nested ternary / `||` chain straddles the line limit.
const wsUrl = globalThis.__ET_WS_URL ||
  `${(typeof location !== "undefined" ? location.protocol : "ws:") === "https:" ? "wss:" : "ws:"}//${
    typeof location !== "undefined" ? location.host : "localhost:8080"
  }/ws`;
```

```js
// Good: lift the parts to named locals; each line stays well under 120.
const loc = typeof location !== "undefined" ? location : null;
const wsProto = loc?.protocol === "https:" ? "wss:" : "ws:";
const wsHost = loc?.host ?? "localhost:8080";
const wsUrl = globalThis.__ET_WS_URL || `${wsProto}//${wsHost}/ws`;
```

If you really cannot get the line under 120 (rare in practice), the next
escape is `// dprint-ignore` on that single line — but verify oxfmt also
leaves it alone after dprint runs.

## Linter ignores: keep in sync with .gitignore

Some linters walk the working tree directly and never read `.gitignore`. When
you add a path to `.gitignore`, update their ignore lists too:

- **lychee** — `config/lychee.toml`'s `exclude_path`. Its gitignore filter
  exists but doesn't cross pnpm's symlink farm under `node_modules/.pnpm/`.
- **ls-lint** — `config/ls-lint.yaml`'s `ignore`. Patterns are gitignore-style
  globs; bare names match only top-level — use `**/<name>` for nested matches
  (e.g. pnpm puts `node_modules` under each package dir, not the repo root).

A new linter that surfaces ignored paths in its output belongs on this list.

## Do NOT add gitignored paths to ec / dprint / typos config

`editorconfig-checker` (`ec`), `dprint`, and `typos` already honor `.gitignore`
on their own. If a path is in `.gitignore`, those three linters skip it — full
stop. Do **not** add a redundant exclude to `.editorconfig`'s `[path/**]`
blocks, `config/dprint.jsonc`'s `excludes`, or `config/typos.toml`'s
`extend-exclude` "just to be safe" or "to be explicit". A redundant exclude is
a lie — it implies the path needs special handling when in fact `.gitignore`
already covers it, and the next reader has to grep two places to understand
the same fact.

The right move when one of these linters flags a generated / vendored / build-
output tree:

1. Add the path to `.gitignore` (and run `mise run gen:dockerignore` if it
   also belongs in `.dockerignore`).
2. Stop. Do not touch `.editorconfig` / `dprint.jsonc` / `typos.toml`.

The narrow exception: a path that **must** stay tracked in git (so cannot go
in `.gitignore`) but still needs the linter to ignore it. `generated/` trees
that we commit (`generated/python-rest/`, `generated/python-ws/`, etc.) are
the canonical case — see the existing `[generated/python-rest/**]` block in
`.editorconfig` for the shape. Reach for a config-file exclude only after
confirming gitignoring isn't viable.

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
