# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Scratch work stays inside this repo

Any throwaway file the agent needs while working -- backup copies of files before destructive edits, generated
probe scripts, captured tool output, intermediate diff materials, anything -- must live under this repo's `target/`
directory (which is already gitignored). Do **not** write to `/tmp`, `/var/tmp`, `~/Desktop`, `~/Downloads`,
`~/scratch`, or any other path outside this working directory. `target/scratch/` is fine; create subdirectories
under it freely and clean up when done.

### NEVER reference a file that isn't in this repo

Do not mention, link to, or write a path to any file that is not tracked in this repo -- not from a tracked file
(source, `*.md`, config, a comment) and not in a commit message, PR body, or code-review note. A relative path
that escapes the repo root, an absolute filesystem path, or prose like "see the write-up in the parent directory"
is meaningless to everyone who clones the repo and reads as a dangling pointer. If the thing being referenced
matters to the repo, it must live **inside** the repo; if it deliberately lives elsewhere (the operator keeps it
outside the tree on purpose), then simply never point at it from inside. Creating or editing files outside the
repo is fine when the operator wants that -- this rule is **only** about never referring to out-of-repo files
from within the repo.

## On macOS: use homebrew bash for ad-hoc agent commands

On macOS, every ad-hoc command the agent runs that is **not** a `mise run <task>` invocation -- investigations,
scratch-area probes, multi-line shell loops, anything with command substitution or arrays -- must execute under
homebrew's `bash` (typically `/usr/local/bin/bash` on Intel, `/opt/homebrew/bin/bash` on Apple Silicon), **not**
the session's default zsh and **not** macOS's bundled `/bin/bash` (which is GNU bash 3.2 from 2007 and predates
many modern features).

Invoke it explicitly per command: `/usr/local/bin/bash -c '<script>'` (or the Apple-Silicon path). Mise tasks are
exempt -- they run under their own configured shell (`bash -euo pipefail -c` etc.) which is already correct.

Reason: agents are bad at zsh-specific quoting and expansion rules, and the system bash is too old to behave like the
bash everyone else uses. Pinning to homebrew bash makes ad-hoc shell behavior predictable and matches what mise tasks
already use.

## Ad-hoc agent commands: use mise-managed tools, or recommend adding one

The "Don't depend on host tools" rule further down is written for mise task bodies, but the same discipline applies to
the ad-hoc commands the agent runs while working -- investigations, scratch-area probes, one-off transforms. Prefer the
repo's mise-managed, version-pinned, cross-platform tools -- `coreutils` (uutils multicall), `rg` (ripgrep),
`find`/`xargs` (uutils findutils), `goawk`, and whatever else the `[tools]` tables pin -- over whatever binary happens
to be on the host (`perl`, the host's own `sed`, a random CLI). A host binary may be absent, a different version, or
missing on another OS, and its shell-quoting is the exact fragility the homebrew-bash rule exists to avoid (a mangled
`perl -pi -e` once silently no-op'd its edit and produced a bogus "passing" test result here before it was caught).

For file edits specifically, reach for the Edit/Write tools rather than a stream editor (`perl -pi`, `sed -i`): they
never touch the shell, so there is no quoting to get wrong.

If you genuinely need a tool that is not installed (or is not mise-managed) to do the job well, do NOT quietly fall
back to the host binary -- surface it. Recommend to the user that the tool be added as a mise `[tools]` entry
(prebuilt-binary backend, cross-platform, per the "Tools must work on every OS" rule), and let them decide. A tool
worth using in this repo is worth pinning.

## Polling cadence when monitoring a PR's CI runs

**Watch at job/check granularity, never run granularity.** `gh run list` reports a _workflow run_ as `in_progress`
for as long as any job in its matrix is still going -- even when other jobs in that same run have **already failed**.
Polling only `gh run list` (or filtering to `status == "completed"`) therefore hides live failures and reads as
"still healthy" when the PR is already red. Use `gh pr checks <pr>` as the primary signal: it lists every individual
check (`check (ubuntu-latest, 25)`, `Codacy ...`, `coverage`, external DeepSource/Codacy/Codecov statuses, ...) with
its own pass/fail/pending, so a failed matrix leg or external check shows immediately. Once a job shows `fail`, pull
its failing step with `gh api repos/<owner>/<repo>/actions/jobs/<job-id>/logs` -- that returns the completed job's log
even while the parent run is still `in_progress` (whereas `gh run view --log-failed` refuses until the whole run
finishes). Only after `gh pr checks` is clean is the PR actually green.

When an agent is watching a PR (e.g. via `/loop`), the next-poll delay follows three tiers -- tight at first (catch
fail-fast errors), loose in the middle (long compile/test phases run on a 30-90 min scale), then loose again once
everything's settled (waiting for the user to push):

- **First 5 minutes after a push**: poll every **1 minute**. Most
  startup / preinstall / fail-fast errors surface in this window.
- **Minute 5 to 1 hour**: poll every **5 minutes**. The long phases
  (cargo builds, docker stage builds, full test matrix) finish on this scale; sub-5-min polling here burns the prompt
  cache without catching anything sooner.
- **After 1 hour, OR once all jobs have stopped**: poll every
  **20 minutes**. The polling now is mostly waiting for the next user
  push -- at this cadence the prompt cache misses anyway, so spending it sparingly on a heartbeat is the right trade.
- **Stop 1 hour after all jobs have stopped.** If the user hasn't
  re-pushed by then, they will tell the agent to restart monitoring.
  Stopping is implemented by omitting the next `ScheduleWakeup` call
  (see the `/loop` skill's "To stop the loop" note).

`/loop` dynamic-mode wakeups are bounded [60, 3600] by the runtime, so each cadence maps directly: 1 min ->
`delaySeconds: 60`, 5 min -> `delaySeconds: 300`, 20 min -> `delaySeconds: 1200`.

## Reproduce a CI failure locally BEFORE fixing it

When a CI check fails, reproduce the exact failure on your own machine **before** changing anything, then verify
the fix against that reproduction. Do NOT push a fix you have only reasoned about -- a blind fix that "should work"
wastes a full CI round-trip (10-30 min) per attempt and erodes trust when it lands still-red.

"It passes when I run the task locally" is NOT a reproduction and NOT proof of a fix. CI runs on a **fresh**
environment; your machine carries months of accumulated state that silently masks the failure. The failures that
bite are environment differences your green local run hides:

- **Floating toolchains/tools** (`nightly`, `latest`, unpinned `conda:`/`npm:`): CI installs today's version; yours is
  whatever you installed weeks ago. A green local run just means your cached version predates the regression. To
  reproduce, install the _exact_ version CI used (read it from the job log -- e.g. `rustc -Vv`, the `mise ... @X`
  install lines) and run with that.
- **Stale install artifacts:** old `mise`/package installs leave symlinks (`.../latest`), directories, and even
  bundled headers that a **fresh** install no longer creates or ships. A hardcoded path that resolves locally can be
  a dead path on CI. Reproduce by removing the stale artifact (or `mise uninstall <tool> && mise install`) so your
  layout matches a clean runner, then re-run.
- **OS-specific behaviour:** the failing lane may be macOS/Windows/Linux-specific. Reproduce on that OS (you are on
  one of the tier-1 platforms); if you cannot, say so rather than guessing.

The loop is: read the CI job log for the exact error + the exact versions -> recreate those conditions locally ->
confirm you see the identical failure -> fix -> confirm the fix turns that same local reproduction green -> only then
push. If you genuinely cannot reproduce locally (e.g. a lane you have no access to), say so explicitly and do not
pass off an unverified change as a fix.

## Keep lines <= 120 characters

`editorconfig-checker` (`ec`, wired into `mise run check` via the `editorconfig-check` task) enforces a 120-char
limit on every file the `[*]` rule in `.editorconfig` covers -- which is almost all of them. A small number of
files have explicit overrides (`LICENSE-*` and generated trees under `generated/`), but assume every file you
touch is bound by 120 unless you have specific evidence otherwise.

When writing comments, doc strings, `#[expect(reason = "...")]` reasons, `description = "..."` fields in TOML,
markdown tables, JSON schemas, etc., keep each line under the limit on the first draft rather than relying on a
follow-up fix-up pass. The most common offenders are: long `reason = "..."` strings on lint attributes, JSON
`description` fields, markdown table rows, and CI-task `description` fields.

Under the limit is the floor, not the target -- write prose and comments to **maximise** use of the 120-char width
repo-wide. Fill each line close to 120 before wrapping rather than breaking early at 60-80 chars; a paragraph that
wraps narrowly wastes vertical space and reads as ragged. This applies to every prose surface: `#`/`//`/`--`
comments, Rust doc comments and `reason = "..."` strings, YAML folded `>-` blocks (semgrep `message:` fields, lint
rule messages), markdown, and Rego/policy headers. When you touch a block that wraps narrowly, reflow it to fill
the width.

## Keep everything ASCII

Non-ASCII characters are **banned repo-wide** (enforced by the `no-non-ascii` semgrep rule). Write ASCII on the
first draft -- never paste a glyph and expect a fix-up pass. Use the ASCII spelling instead: `--` for an em-dash,
`->` for an arrow, `...` for an ellipsis, `<=`/`>=`/`!=`/`^2` for math glyphs, and straight `'`/`"` quotes for
curly quotes. ASCII keeps terminals, log scrapers, and `grep` reading the same bytes the editor shows. The only
exemptions are generator-owned trees (`generated/`, `verification/`, `**/HELP.md`, `**/pkg/`), license texts, and
binary assets -- everything you hand-write is bound by the ban.

## No trailing-backslash line continuations

Trailing-backslash line continuations (a line ending in `\` to join with the next) are **banned everywhere** in
this repo. The only exceptions are README files and generated trees (e.g. `verification/`). Character escapes
_inside_ string literals -- `"\n"`, `"\t"`, regex `\d`, a Windows path `C:\Foo\Bar` as a value -- are NOT
continuations and are fine; the ban is on the end-of-line `\` that joins source lines into one logical statement.

If a logical line would otherwise exceed the 120-char limit, pick a no-backslash split. Some patterns that work in the
languages this repo uses:

- **mise task `run` bodies**: factor into shell variables / `[vars]`
  entries / multiple statements. Do this on the first draft (see also the line-limit rule above), not as a fix-up pass.
- **Dockerfile ENV / ARG**: build the value across multiple `ARG`s and
  compose the final `ENV` from them via `${VAR}` expansion. Example
  from `Dockerfile.nanoserver`'s `MISE_DISABLE_TOOLS`:

      ARG MISE_DT_BASE=cargo:dart-typegen,conda:m2-gnupg,...
      ARG MISE_DT_PY=pipx:componentize-py,pipx:...
      ENV MISE_DISABLE_TOOLS=${MISE_DT_BASE},${MISE_DT_PY}

- **Dockerfile `RUN` block**: switch to BuildKit's HEREDOC form
  (`RUN bash <<'EOF'` ... `EOF`) -- each shell command sits on its own line
  with no continuation needed. Three rules, all enforced by
  `config/conftest/policy/dockerfile/dockerfile.rego` + the matching semgrep rule
  under `config/semgrep/`: (1) interpreter is **`bash`, placed BEFORE the
  `<<TAG`** (default `/bin/sh` on Debian/Ubuntu is dash, which rejects
  `set -euo pipefail`; the inverse `RUN <<EOF bash` form is silently
  broken because BuildKit treats trailing tokens as literal); (2) the
  **delimiter must be quoted (`<<'EOF'`)** -- with an unquoted `<<EOF`,
  the outer `/bin/sh -c` that wraps the RUN performs `$(...)` command
  substitution on the body BEFORE bash runs, so `libicu=$(apt-cache ...)`
  is evaluated against the outer shell at the wrong moment (Fedora
  aborted with `apt-cache: command not found`; Debian/Ubuntu ran
  apt-cache before the script's `apt-get update`, getting a stale
  cache); quoting defers every expansion to bash, and ARG values needed
  inside the body must be promoted to ENV beforehand (`ARG FOO=...` ->
  `ENV FOO=${FOO}`); (3) **first body line must be `set -euo pipefail`**
  -- HEREDOC RUNs go through bash without inheriting strict mode from any
  outer setting, so the invariant has to be re-declared inside every
  body. Leave a blank line between the closing `EOF` and the next
  instruction -- hadolint's parser otherwise errors with `unexpected 'E'
  expecting a new line...`.
- **YAML run-bodies**: a `|` block scalar already keeps each shell
  line natural; no continuations are necessary.
- **Long flag lists**: drop them into a config file (`.env`, `.cfg`,
  task `[vars]`) or split into one flag per line inside a HEREDOC /
  block scalar.

If a split honestly isn't possible without `\`, stop and surface the constraint -- do not reintroduce the
regression. Existing `\` uses in the repo (Dockerfile RUN-block multi-liners, etc.) are regressions slated for
cleanup, not precedent.

## Document each thing exactly once

Document each thing **once**, in the single place it is most relevant -- the code, config entry, or task it describes
-- and **nowhere else**. Do not restate the same explanation in a second comment, in the README, in this file, or in a
commit message that competes with it.

Do not even add a pointer to where something is documented ("see X", "as described in Y", "(documented in Z)").
Such cross-references are themselves duplication: they go stale, and they multiply the places that must change
when the thing changes. Trust the developer to find the relevant documentation themselves -- they know how to
read the code, `grep`, and follow the obvious file.

When you find yourself about to explain something that is already explained elsewhere, stop: either the existing spot
is the right home (so say nothing here), or this is the better home (so move it here and remove the original). One
canonical location, never two.

## Don't narrate dependency version changes in comments

A comment describes what the code does now, not how it got there. When you bump a dependency and update code to its
new API, do **not** add a comment explaining the change -- no "0.4 renamed `Foo` to `Bar`", no "replaces the old
`x()`/`y()` calls", no "the 0.3 API used to ...". The renamed symbol and the new call are already visible in the code,
and git history records what changed, so the note is pure duplication that goes stale the moment the next version
lands. Comment the current code as if the old version never existed.

The narrow exception is a version fact that is load-bearing -- it explains something that would otherwise read as
arbitrary or wrong and that a reader needs in order not to break things: a pin deliberately held back for
compatibility, or a workaround for a specific upstream bug. "The implementation changed but the behaviour did not"
never qualifies. When in doubt, leave it out.

## Common Commands

All tasks run through `mise run <task>`. The aggregates below act on Rust + the universal checks
**plus whichever guest languages `MISE_ENV` has loaded**; use the `*-all` variants (`check-all`, `test-all`,
`build-modules-all`, `gen-specs-all`) -- or set `MISE_ENV` -- to cover every language.

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

**Build a single module:** `MISE_ENV=<lang> mise run build-ws-<module>-module` (e.g.,
`mise run build-ws-face-detection-module` for the Rust modules, or `MISE_ENV=zig mise run build-ws-zig-data1-module`).

## Formatters & checks by file type

**Verify the change actually works before running the lint/format battery.** Functional verification comes first:
run the affected tests, exercise the endpoint, or load the page in the browser and confirm the behaviour is right.
Only then move on to the formatter/check passes below. Linting a broken change wastes the whole battery -- the
functional fix that follows (a renamed symbol, a rewritten block, a CSS edit discovered only in the browser) dirties
the files again and every check has to be re-run. Green checks on code that doesn't work are worthless.

**Agents must not run `mise run check` (or `check-all`).** They are aggregate gates intended for CI and humans --
running the whole battery for every edit is slow, expensive, and almost always wasteful when only a couple of file
types changed. Pick the targeted tasks from the tables below that match the extensions of the files you actually
modified, and run only those. Same goes for `fmt`: run the per-file-type formatter, not the aggregate.

**Always run `mise run semgrep-check` before finishing a set of changes to any files** -- it is the one check to
run regardless of which file types you touched. It carries repo-wide, cross-cutting rules that the per-file-type
tasks don't (comment summary lines across TOML/YAML/JSON/C, the `no-non-ascii` ban, banned constructs in
`generic` mode over config text), and it is cheap. Skipping it is how comment-style and cross-file regressions
slip through to CI.

**Agents must not invoke formatter/linter binaries directly** (e.g. `mise exec -- taplo format`, raw
`taplo`/`dprint`/`oxfmt`/`cargo fmt` calls). Always use the corresponding mise task (`mise run taplo-fmt`,
`mise run taplo-check`, `mise run dprint-fmt`, etc.). The tasks carry the project's config-file paths
(`config/taplo.toml`, `config/dprint.jsonc`, ...), exclusion lists, and flag conventions; bypassing them produces
results that don't match the canonical pipeline (different excludes, different `reorder_keys`/`column_width`
settings, different file globs). taplo specifically: the only right way to format TOML in this repo is
`mise run taplo-fmt`.

The `mise run <task>` formatters and checks per file type. The aggregates (`fmt`/`check`/`fmt-all`/`check-all`)
run every loaded language's row; guest rows need their `MISE_ENV` loaded.

| File type      | Formatter task(s)               |
| -------------- | ------------------------------- |
| `*.rs`         | `cargo-fmt`, `cargo-clippy-fix` |
| `*.toml`       | `taplo-fmt`                     |
| `*.py`         | `ruff-fmt`                      |
| `*.dart`       | `fmt:dart`                      |
| `*.zig`        | `fmt:zig`                       |
| `*.c`, `*.cpp` | `clang-format`                  |
| `*.cs`         | `fmt:dotnet`                    |

| File type      | Check task(s)                                                                                  |
| -------------- | ---------------------------------------------------------------------------------------------- |
| `*.rs`         | `cargo-check`, `cargo-clippy`, `cargo-fmt-check`, `cargo-doc-check`, `ast-grep-check`          |
| `*.toml`       | `taplo-check`, `conftest-check-toml`, `semgrep-check`                                          |
| `*.yaml`       | `ast-grep-check`, `conftest-check-yaml`, `ryl-check`, `action-validator-check`, `zizmor-check` |
| `*.json`       | `semgrep-check`                                                                                |
| `*.py`         | `check:python`                                                                                 |
| `*.dart`       | `check:dart`                                                                                   |
| `*.zig`        | `check:zig`                                                                                    |
| `*.c`, `*.cpp` | `clang-format-check`, `clang-tidy-check`, `cpplint-check`                                      |
| `*.cs`         | `check:dotnet`                                                                                 |
| `*.java`       | `check:java`                                                                                   |

`dprint-fmt` / `dprint-check` cover `*.md`, `*.yaml`, `*.json`/`*.jsonc`, `*.ts`/`*.js`, `*.css`, `*.html`,
`*.java`, and `Dockerfile*`; `hadolint-check` also lints Dockerfiles, and `link-check` scans `*.md` + `*.rs`. Every
file is covered by `editorconfig-check` and `typos-check`, file and directory names by `ls-lint-check`, and `*.yml` is
rejected by `semgrep-check` (use `*.yaml`).

For Rust inner-loop iteration on a single crate, use `mise run cargo-clippy-check-pkg <package>` (alias
`clippy-pkg`) instead of `cargo-clippy-check` -- it runs `cargo clippy --keep-going --tests -p <package>` so you
only compile that crate plus its deps, not the whole workspace. Same lint config; just narrower scope. Switch back
to `cargo-clippy-check` for the final verification pass before declaring done, since cross-crate
`feature-unification` differences can fire workspace-wide lints that don't fire on a single `-p` build.

## Architecture

This is a WebSocket-based edge computing framework.

The server is a hub: it maintains an agent registry, routes messages between agents, provides agents with storage, and
serves node module packages as static files to browsers.

### WebSocket protocol

The Rust sources of truth are `ClientMessage` (what clients send) and `ServerMessage` (what the server sends) in
`libs/edge-toolkit/src/ws.rs`. The full message catalogue (every wire `type`, request/response shape, and shared
schema) is regenerated by `mise run gen:ws-spec` into [`generated/specs/ws.yaml`](generated/specs/ws.yaml)
(AsyncAPI 3.0). Generated language clients sit next to it under `generated/dart-ws/`, `generated/python-ws/`, and
`generated/specs/wit/deps/et-ws-messages/`.

The server acts as a pure hub for **unrecognised** frames: any text the server can't parse as a known
`ClientMessage`, and any binary frame, is forwarded verbatim to every other connected agent (with a single
`info!` log per broadcast). This lets agents use arbitrary out-of-band payloads without needing a server-side
enum entry. Explicit `et-broadcast-message` still wraps payloads in an `et-agent-message` envelope as before.
Both paths require the sender to be a connected agent; frames from unassigned clients are dropped.

### REST surface

Every HTTP endpoint exposed by ws-server (health probe, module discovery, module assets,
per-agent storage) is annotated with `#[utoipa::path]` in its handler. `mise run gen:ws-spec`
emits the aggregated [`generated/specs/rest.yaml`](generated/specs/rest.yaml) (OpenAPI 3.0)
and the typed Rust client at [`generated/rust-rest/`](generated/rust-rest/) via `progenitor`
-- consumed by `et-ws-wasi-runner` (native) and the browser `data1` module (WASM). The client
crate's `tracing` feature is on by default (W3C `traceparent` injected on every request via a
progenitor pre-hook) and off in WASM consumers, which switch reqwest to its `fetch()` transport.

### Services (`services/`)

- **ws-server** -- Actix-web entry point; wires together the four concerns below. Loads registry from
  `registry.yaml` on startup and saves it on shutdown. Run with `mise run ws-server`.
- **ws** (`et-ws-service`) -- Agent registry and WebSocket hub. The registry tracks agent state and
  queues pending direct messages. Registry is persisted to `registry.yaml`.
- **storage** (`et-storage-service`) -- File storage for agents, mounted into the app as configured by `StorageConfig`.
- **modules** (`et-modules-service`) -- Scans configured paths for directories containing `package.json`,
  then serves those files statically. The root module (default UI) is the package named by `ModulesConfig::root`.
- **ws-wasm-agent** -- Browser-side WASM client that connects back to the server over WebSocket.

### Modules (`services/ws-modules/`)

Node module packages served as static files by `et-modules-service`. Each module has a `package.json`
with a `main` JS entry point. Built artifacts land in `pkg/`. The browser loads and runs these.
The server only serves them from disk.

Languages:

- **Rust -> WASM** (wasm-pack): audio1, bluetooth, comm1, data1, except1, face-detection, geolocation, graphics-info,
  har1, math1, nfc, sensor1, speech-recognition, video1
- **JavaScript**: js-data1 (esbuild bundle of the AWS SDK v3 twin), js-math1 (dependency-free, committed as-is)
- **Dart -> JS**: dart-comm1, dart-data1, dart-math1
- **Kotlin -> WASM (WasmGC)**: kotlin-data1, kotlin-math1 -- compiled by the Kotlin Gradle plugin's `wasmJs` target;
  each module is a WasmGC binary (browser GC manages the Kotlin heap), so it needs a WasmGC-capable engine
- **Python (Pyodide)**: pydata1, pyeye1, pyface1, pymath1
- **C# (.NET WASM)**: dotnet-data1, dotnet-math1
- **Java (TeaVM -> JS)**: java-data1, java-math1 -- both built by the single root `pom.xml` (one compilation, one
  teavm-maven-plugin execution per module)
- **R (webR -> WASM)**: rdata1, rcomm1, rmath1 -- browser-only (webR spawns a classic Worker, unsupported by Deno's
  ws-web-runner). Their JS shims are linted by the `js` env; `MISE_ENV=r` supplies webR + the vendoring build tasks.
- **Zig -> WASM**: zig-data1, zig-except1 (C++ wasm-exception-handling demo), zig-math1

The math1 modules are one family, driven by a storage exchange: a fake agent (the shared helper in
`et-ws-test-server`, embedding the canonical committed input at its `data/math1-input.json`) injects the
input JSON into ws-server storage and broadcasts a `math1-input` pointer over the hub; each module reads the
input, runs the same FedAvg simulation (`+ - * /` on f64 only), and stores its global model to
`math1-output.json` in its own bucket, which the test harness reads and verifies against the expected
weights -- proving bit-identical float math across every guest runtime. Beyond the browser twins the family
also covers wasi-math1 (a WASI Preview 2 component under `et-ws-wasi-runner`'s wasmtime) and the `math1.py`
module under `et-ws-pyo3-runner` (native CPython) -- the two non-browser executors.

- **Python (componentize-py -> WASI Preview 2 component)**: wasi-graphics-info -- runs in
  `et-ws-wasi-runner` rather than the browser. The WIT world the component implements is at
  `services/ws-wasi-runner/wit/world.wit` and is mirrored under the module's own `wit/`.
  Drives two standardised WASI interfaces end-to-end: (1) `wasi:webgpu/webgpu@0.3.0-rc.2` for a real
  4x4 compute matmul through a host wgpu device, and
  (2) `wasi:nn/{graph, tensor, inference}` for MNIST inference. Bundles `mnist-12.onnx` (served
  from `pkg/` as a static asset; the guest fetches it via the `storage` host import because
  componentize-py 0.23 doesn't bundle non-Python data files), then runs inference through ONNX
  Runtime via `wasi:nn/graph.load` + `inference.compute` and verifies the predicted class.

### Libraries (`libs/`)

- **edge-toolkit** -- Common utilities, config, serialization (shared across services)
- **web** -- WASM web helpers (Canvas, MediaStream, WebSocket bindings for browser modules)

### WASI Runner (`services/ws-wasi-runner/`)

`et-ws-wasi-runner` -- runs ws-modules compiled to **WASI Preview 2 components** (rather than
browser WASM modules). It fetches the module's `pkg/package.json` from the ws-server, downloads
the `.wasm` named by the `wasi-main` field, instantiates it under `wasmtime` with async support,
and calls the exported `entry.run` function.

Host imports (defined in `wit/world.wit`, package `et:ws-wasi@0.1.0`):

- `log` -- `log` and `set-status` for guest output
- `clock` -- `sleep-ms`, `now-ms`
- `storage` -- `put-file`/`get-file` proxied to the ws-server's storage service via reqwest
- `ws` -- websocket client backed by `tokio-tungstenite`; mirrors the wire format of
  `et-ws-wasm-agent` so events look the same on the server

Plus, attached to the same Linker but defined by external WIT packages:

- `wasi:webgpu/webgpu@0.3.0-rc.2` -- the full upstream interface, implemented by the
  `wasi-webgpu-wasmtime` crate from the wasi-gfx project. The runner supplies only a
  `wgpu_core::global::Global` handle and the resource table, via `WasiWebGpuCtxView` on `HostState`;
  everything else (every resource, every method, render paths included) is upstream's. The WIT is
  fetched verbatim into `generated/specs/wit/deps/wasi-webgpu/` by `fetch-wit-deps`, not trimmed.
  This interface is where the `async` on `entry.run` comes from: `request-adapter`, `request-device`
  and `map-async` are `async func`, a component can only await an async import from an async export,
  so every WASI guest's entrypoint carries the async ABI and the host drives it through
  `Store::run_concurrent`.
- `wasi:nn/{tensor, graph, inference, errors}` -- standardised ML inference. The host wires
  `wasmtime-wasi-nn` with the ONNX Runtime backend (`ort` 2.0.0-rc.10, pinned because rc.11+
  moved API surface that wasmtime-wasi-nn 47 still uses). Guests load model bytes via
  `graph.load`, build `Tensor`s, and call `compute` -- the same shape of calls Spin / wasmCloud
  / Fermyon production wasi-nn workloads use. CUDA dispatch is opt-in via the runner's
  `cuda` cargo feature (`cargo build -p et-ws-wasi-runner --features cuda` or
  `RUNNER_FEATURES=cuda mise run ws-wasi-runner`); the default build is CPU-only because
  Pyke's `ort` download-binaries CUDA prebuilt only exists for some triples (notably
  Linux x86_64). CoreML-on-macOS would need a wasmtime-wasi-nn patch (its `onnx.rs` only
  knows the CUDA provider for `ExecutionTarget::Gpu`).

`RUNNER_MODULE` selects the module (e.g. `wasi-graphics-info`). `WS_SERVER_URL` defaults to `ws://localhost:8080/ws`.

### Utilities (`utilities/`)

- **cli** (`et-cli`) -- Scenario and module tooling. Reads scenario YAML and outputs `mise.toml` or `compose.yaml`;
  also generates module `pkg/package.json` files with `et-cli module-package-json`.
  Deployment-specific generators live under `utilities/cli/src/deployment_types/`.
  Module package JSON generation lives under `utilities/cli/src/module_package_json/`.
- **int-gen** (`et-int-gen`) -- Internal code generator emitting artifacts under `generated/` from in-repo Rust
  sources of truth (AsyncAPI/OpenAPI YAML, WIT, KDL, schema JSON, the typed Rust REST client, the Zig client).
- **onnx** (`et-onnx`) -- ONNX model utilities.

Each utility has a committed `HELP.md` (under its crate dir) that mirrors the clap-derive tree via the
`markdown-help` feature + hidden `--markdown-help` flag. **Read `utilities/<name>/HELP.md` to learn what
the CLI does -- don't run `cargo run -p <name> -- --help`** (much slower: cargo has to build the binary
first; HELP.md is the same content as a static file). `mise run gen-help-all` regenerates them all; the
`gen-help-check` task (wired into `check:rust`) fails on drift.

### Verification (`verification/`)

Scenario YAML inputs live in `verification/*/input/`. Expected outputs are checked into `verification/*/output/`
and must stay in sync -- `mise run check` will fail if they drift. Regenerate with `mise run regen-verification`.

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

Tests must live in a `tests/` directory or in source files prefixed `test_`. Do not use inline `#[cfg(test)]` modules.
If a function is private but needs testing, add a `[lib]` target to the crate and export it so `tests/` can reach it.

Every file under `tests/` must start with `#![cfg(test)]` (placed after the file's `//!` doc comment, if any).

Shared, **low-dependency** test helper functions -- free-port reservation, port-readiness waits, and the like --
belong in the `et-test-helpers` crate (`libs/test-helpers`); reuse and extend it rather than re-implementing the same
helper per test. Keep its dependency footprint small (currently just `port_check` + `retry`): anything heavier or
domain-specific gets its own test-support crate instead (e.g. `et-ws-test-server` for an in-process ws-server, or
`int-otlp-mock` for a mock OTLP collector).

### NEVER skip, ignore, or platform-disable a test without explicit user approval

A test that doesn't run is worse than no test: it reads as coverage while asserting nothing. Do not add `#[ignore]`,
`#[cfg_attr(<cond>, ignore = "...")]`, an early `return` guard, a `cfg`-out, or any other skip to a test (or disable a
feature) without the user explicitly approving that specific skip. Two rationalisations are specifically banned because
both are false under this repo's design:

- **"It might not work on Windows (or some OS) and I can't verify it here."** Every one of the five supported platforms
  (macOS arm64/x64, Linux x64/arm64, Windows x64) is a first-class target, and CI runs the full matrix. Deciding whether
  a test passes on an OS you can't run locally is **CI's job, not yours** -- write the test to run everywhere and let CI
  report. Do not `#[cfg_attr(windows, ignore)]` (or similar) on the guess that it "wants a check first". If CI later
  proves a genuine, understood platform incompatibility, that is the moment to gate it -- narrowly, with the CI evidence
  cited (per the "Workarounds" rules below), and still only with user sign-off.
- **"A required tool/binary may not be installed."** The README mandates `mise`; every sanctioned environment (CI,
  Docker images, a contributor workstation) has every `[tools]` binary on `PATH`. A test must never self-skip because
  a tool "might be missing" -- a missing mise tool means a misconfigured environment, which must **fail loudly**, not
  silently pass. Spawn the binary and let the `.expect(...)` panic surface the misconfiguration.

If you believe a skip is genuinely warranted, stop and ask the user; do not add it pre-emptively.

### Known intermittent CI failure: pyo3-runner torch registration timeout

`et-ws-pyo3-runner`'s `module_behaves::case_5_torch` intermittently fails on the Linux, Windows, and macOS lanes
of test.yaml and build.yaml with the literal failure line

    Error: "runner never registered"

arriving a few seconds after torch's cold first import prints
`UserWarning: Failed to initialize NumPy: No module named 'numpy'` (from
`torch\_subclasses\functional_tensor.py:362`, a benign warning) and its
`cpu = _conversion_method_template(device=torch.device("cpu"))` line -- i.e. the spawned runner was still inside
torch's import when the test's registration timeout expired. The ~117 MB `pipx:torch` package's first import on a
cold runner is the slow step; a rerun passes because the import caches warm. Observed on commit
`6479913bdc288dd680fbe0520f63054e8c71fe6c` at
`https://github.com/edge-toolkit/core/actions/runs/28686533955/job/85080173283` (PR #70; the rerun passed and the PR
merged), and on the `default (macos-latest, 45)` job -- same signature, unix-form
`torch/lib/python3.13/site-packages/torch/_subclasses/functional_tensor.py:362` warning path -- on commit
`f2a85307a2415f4a50625627795e02c84f1c8e5d` at
`https://github.com/edge-toolkit/core/actions/runs/28865327221/job/85613919558` (PR #73), 20.68s into the test
run. Recurred at commit `1e323acb7fc433b66efed904b3b30ab3216dbf90` (PR #76) on three lanes of one run at once --
the first Linux sighting, and the first time it took more than a single lane of a commit: build.yaml's
`build (ubuntu:22.04)` (~15.76s) at
`https://github.com/edge-toolkit/core/actions/runs/29034248895/job/86174973520`, and test.yaml's `override (mingw)`
(~18.13s) and `override (msvc)` (~18.96s) at
`https://github.com/edge-toolkit/core/actions/runs/29034248854/job/86174895528` and
`https://github.com/edge-toolkit/core/actions/runs/29034248854/job/86174895194`. Three simultaneous same-commit
failures read as the cold torch import consistently overrunning the timeout rather than an occasional flake -- so
the root-cause fix below is now due, not optional. If this signature recurs, stop rerunning and fix the root cause:
raise (or make torch-case-specific) the
runner-registration timeout in the pyo3-runner module tests, or warm the torch import before the registration clock
starts.

### Known intermittent CI failure: mise tool-install `api.github.com` attestation/metadata flake

`mise install` (in the install-mise-tools composite action) intermittently fails on the Windows lanes when its
calls to `api.github.com` -- GitHub artifact-attestation verification and release-metadata lookups -- get
rate-limited or time out. The captured signature is a tool install aborting on the attestation endpoint, e.g.

    mise ERROR Failed to install github:uutils/findutils@0.8.0: GitHub artifact attestations verification
    error ...: HTTP error: error sending request for url
    (https://api.github.com/repos/uutils/findutils/attestations/sha256:...)

usually preceded by several retried `mise WARN HTTP GET https://api.github.com/repos/<owner>/<repo>/releases...`
lines. When it strands the install, the composite's retry step can then trip the separate busybox/Git-Bash
`cygheap read copy failed` fork pathology and the job hits its action timeout instead of a clean error. Observed on
the `override (mingw)` job at commit `396aa98d24e4a945528cdbac33fbc61b66831e8a`,
`https://github.com/edge-toolkit/core/actions/runs/28698136445/job/85111320237` (a rerun of the same commit
installed cleanly). This is an api.github.com rate-limit/transient-network flake, not a repo defect -- a
`GITHUB_TOKEN` is already forwarded to raise the ceiling. If it becomes frequent rather than occasional, the
durable fix is to mirror the affected assets via the upstream-cache pattern (which fetches from our own release
CDN, off the api.github.com attestation path) rather than adding a retry wrapper.

### Known intermittent CI failure: vector_otlp_relay store-and-forward timing

`et-ws-wasi-runner`'s `vector_otlp_relay::vector_relays_buffered_otlp_after_backend_comes_online` intermittently
fails with the literal panic line

    mock never received the relayed `relay-probe` span -- store-and-forward failed

from `services/ws-wasi-runner/tests/vector_otlp_relay.rs`. The test pushes one OTLP trace into Vector while the
sink target is dead, brings a mock collector up on the sink port, then polls the mock for the redelivered span.
The flake is a redelivery-latency race, not a store-and-forward defect: Vector retries the dead sink with an
exponential backoff (`retry_initial_backoff_secs=1`, doubling; `retry_max_duration_secs=300` in
`config/vector-otlp-relay.yaml`), so when its first attempts land in the gap between the mock being told to start
and its listener actually accepting, the next retry can be tens of seconds out -- and at the original 30s poll
ceiling that occasionally overran, so the span arrived just after the test gave up. Observed on the `default
(windows-latest)` lane at commit `71e70e56946abefcea6daf47a7745e5f8f186dad`,
`https://github.com/edge-toolkit/core/actions/runs/28923326008/job/85829470834` (the failure is OS-agnostic --
any cold/contended runner, macOS included, can hit it). Fix applied: `wait_for_relayed_span` now polls ~120s
(`Fixed::from_millis(250).take(480)`) instead of ~30s, which the poll exits the instant the span lands so the
happy path is unaffected. If this signature recurs at the wider ceiling, the redelivery is being delayed past
120s -- root-cause it rather than widening again: cap Vector's retry backoff interval (or make
`int_otlp_mock::start_on` block until its listener is truly accepting so Vector's early retries don't grow the
backoff), don't just bump the poll.

## Workarounds

When you can't (or shouldn't) fix the root cause right now -- a libc race in an upstream dep, a flaky platform driver,
a runner-image quirk, a toolchain bug, a CI-only flake -- and you decide to paper over it with a workaround:

- **Gate narrowly to the affected situation.** Use `#[cfg(target_os = "...")]`
  / `target_arch` / `target_env` (or the equivalent in YAML / Dockerfiles /
  build scripts) so other platforms keep exercising the real path. Don't
  blanket-disable.
- **Gate to only the use site that needs it.** For test-only quirks, a
  test-set env var the production code checks works well. For build-time quirks, a feature flag or build profile. The
  default behaviour on every platform should stay the unworkarounded one wherever it works.
- **Embed the exact error message verbatim** at the workaround site -- a
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

## NEVER disable anything on Windows without explicitly asking the user first

Windows x64 is a first-tier platform with exactly the same standing as Linux x64 and macOS arm64 -- not a
best-effort port. Any change that turns functionality off on Windows needs the user's explicit sign-off, requested
BEFORE the change is made: `#[cfg(windows)]`-ing code out, excluding a crate from a Windows build or test run,
os-scoping a mise tool away from Windows, skipping a task/check/lint on the Windows lane, weakening a Windows code
path to a stub, or shipping a feature that silently no-ops there. This holds no matter how small or "temporary"
the disable is, and no matter how red CI is -- a broken Windows lane is a bug to root-cause, not evidence that
Windows support is optional.

"It fails on Windows and works everywhere else" is the START of an investigation, not a justification for turning
the feature off. The failure almost always has a specific, fixable Windows cause -- a cmd.exe or PATH-length
quirk, a shim difference, a missing prebuilt for the target triple, a path-separator or encoding assumption -- and
the fix keeps the functionality on. If, after root-causing, you still believe a Windows-scoped disable is
genuinely the right call, stop, present the evidence and the exact proposed scope, and wait for the user to
approve it explicitly. Do not bundle an unapproved Windows disable into a larger change, and do not widen one that
already exists.

## Non-negotiable platform constraints

Project decisions pinned here are not subject to "easier path" rewrites, even when something downstream is in the way.
Don't propose disabling or working around them; find a compliant solution instead.

1. **`Dockerfile.nanoserver`'s base image stays Nano Server.** Don't switch
   to Windows Server Core / LTSC / any non-Nano-Server base, no matter how much it would unblock a tool. Minimal image
   size on the Windows lane is load-bearing.
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

Five supported platforms in two tiers. **Main tier:** macOS arm64, Linux x64, Windows x64 -- every tool in the
`.mise/config*.toml` `[tools]` tables must use a prebuilt-binary backend (aqua/github/http) here; a `cargo:`
source-build isn't acceptable. **Second tier:** Linux arm64, macOS x64 -- every tool must still install and run,
but slower install mechanisms are allowed because release authors often skip prebuilts for these arches: a
`cargo:` source-build (or alternate backend) `os`-scoped to a second-tier-only platform is fine. The conftest
mise policy (`config/conftest/policy/mise/mise.rego`) enforces both rules, including the narrow per-name allowlist for
tools that have no prebuilt at any triple.

Skipping a tool entirely with `MISE_DISABLE_TOOLS` is reserved for `Dockerfile.nanoserver`, where trimming the
image to just what its build needs is expected. Other Dockerfiles may only disable tools that are unused by the
build system anyway (e.g. `cargo-expand`, a dev-only macro-debugging tool).

## Installing `cargo:` tools on Windows: pin an msvc `install_env`

The Windows Rust target here is `x86_64-pc-windows-gnullvm` (`config.windows.toml`'s `[env] CARGO_BUILD_TARGET`),
which cargo-binstall reads to decide which prebuilt to fetch. Almost no project -- and neither cargo-quickinstall --
publishes `*-gnullvm` binaries; they ship `x86_64-pc-windows-msvc`. So a bare `cargo:<tool>` on Windows makes
binstall look for a gnullvm prebuilt, find none, and fall through to a slow `cargo install` source build (which
often then fails on missing gnullvm `rust-std`). This has been rediscovered repeatedly (cargo-expand, wasm-opt,
action-validator) -- if a `cargo:` tool source-builds or "not found"s on the Windows lane, this is why.

When adding a `cargo:` tool that must work on Windows, declare it in `config.windows.toml` with a per-tool msvc
override so binstall fetches the msvc prebuilt (an msvc binary runs fine on the gnullvm host -- the target only
affects what the tool links, not what executes it):

    "cargo:<name>" = { version = "latest", install_env = { CARGO_BUILD_TARGET = "x86_64-pc-windows-msvc" } }

First confirm an msvc prebuilt actually exists for that `<name>@<version>` -- check cargo-quickinstall's release
tags for `<name>-<version>-x86_64-pc-windows-msvc.tar.gz` -- then add the tool to `mise.rego`'s
`allowed_cargo_no_prebuilt`; that conftest allowlist is the gate that forces you to have verified a binary exists.
If no msvc prebuilt exists either, use a different backend (aqua/github/http, or mirror it via the upstream-cache
pattern), or os-scope the tool off Windows and provide coverage another way (as `dart-typegen` does via
`http:dart-typegen`).

As of mise 2026.7.1 the `cargo:` backend is broken on the Windows lane in a further way: `cargo-binstall` itself
fails to execute, so the source-build fallbacks above never even get a chance. `cargo:open` (a low-dep pure-Rust
opener with no prebuilt anywhere) surfaced it:

    mise ERROR Failed to install cargo:open@latest: failed to execute command: <mise>\shims\cargo-binstall
    -y open@5.3.6 --locked --root <mise>\installs\cargo-open\5.3.6: %1 is not a valid Win32 application. (os error 193)

os error 193 is ERROR_BAD_EXE_FORMAT -- the staged cargo-binstall is not a runnable Win32 image (a mise
install-path / shim fault, not an `open`-specific one), so right now no `cargo:` tool installs on Windows at all.
Until mise fixes it, keep new `cargo:` tools off the Windows lane -- os-scope them to `["linux", "macos"]` where
Windows never needs them, or add the tool to `MISE_DISABLE_TOOLS` on every Windows surface that runs `mise install`.
The `install-mise-tools` composite action sets it for the CI jobs, and the Windows Dockerfiles carry it in their own
`MISE_DISABLE_TOOLS`. `cargo:open` takes the latter route: its open-o2 opener is a known Windows gap until the
backend works again.

## NEVER read or edit `.github/workflows/upstream-cache.yaml` without explicit permission

Agents are not allowed to open, read, or modify `.github/workflows/upstream-cache.yaml` unless the operator has
explicitly permitted that specific access in the current conversation. This workflow publishes binary artifacts
to the project's GitHub releases; unsupervised changes to it are a supply-chain risk, so it is maintained by the
operator only. If a task appears to need a new cached asset or a change to an existing job, describe what is
needed and stop -- do not touch the file, and do not add the surrounding pieces (bootstrap task, `data.toml`
entry, tool URL swap) that would depend on it.

## Adding a new `upstream-cache` entry

When upstream has no prebuilt binary for a platform we target (the case the "Tools must work on every OS" rule
needs help solving), we mirror the build to our own GitHub release and point `[tools."http:<name>"]` at it. The
pattern is the same for every cache entry; copy the `rustpython` / `augeas` / `dart-typegen` / `gnupg-w32` shape:

1. **Bootstrap task in `.mise/config.maint.toml`.** An idempotent
   `[tasks.bootstrap-<name>-release]` that runs `gh release view <tag>
   ... || gh release create <tag> ... --prerelease`. The `--notes`
   field must contain **only the upstream project URL and its SPDX
   license expression, separated by a newline -- nothing else** (no
   prose, no "auto-built by..." footer). The maintainer runs this
   once per repo before the workflow's first dispatch.

2. **Job in `.github/workflows/upstream-cache.yaml`.** Mirrors the
   rustpython job's shape: a `Detect missing <name> asset` step that
   queries the release via `gh release view --json assets`, gates the
   build/upload steps on `outputs.work == 'yes'`, then a build step,
   then a publish step that runs `gh release upload <tag> --clobber`.
   The job creates **no** release -- the bootstrap task does. The job
   uploads only.

   **Use pkgx's pantry recipe as the canonical source of build
   instructions.** Before writing the build step, fetch
   `https://github.com/pkgxdev/pantry/blob/main/projects/<upstream>/package.yml`
   (or the closest match -- pkgx organizes by upstream domain, e.g.
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
   publish task auto-edits config.toml -- same pattern is fine for new
   entries.) The `checksum` is specific to THIS pattern -- a mirror we
   control and publish a sidecar for. A plain `http:` tool that points
   straight at a stable third-party URL (not one of our mirrors) may
   omit `checksum`: mise's http backend treats it as optional, and no
   policy requires it (`checksums.rego` governs only the `data.toml`
   mirror assets, not `[tools."http:*"]` entries). `http:openobserve`
   is such a direct-upstream tool and carries no checksum.

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
   `<name>_asset` <-> `[asset.<filename>]`) and the per-entry shape
   (`url` + `upstream` + `license` required; `sha256` present,
   optionally empty during bootstrap). Bump all rows in lockstep
   when the upstream version changes.

6. **Triggers.** `pull_request` on `paths: [.github/workflows/
   upstream-cache.yaml, .github/actions/install-mise/**]` so editing
   the workflow exercises the job before merge; `workflow_dispatch`
   for ad-hoc rebuilds (e.g. version bump). No `push:` trigger -- we
   don't want a `main`-merge to rebuild assets.

Required side-effect: an `etc-` entry in `config/conftest/policy/mise/mise.rego`'s allowlist of `http:` tools that have
no prebuilt at any triple, if applicable. (Skip if the tool is OS-scoped and the allowlist already covers it.)

## Fetch resilience: prefer upstream-cache, not retry wrappers

When a network fetch in a task body (rclone, cargo fetch, raw curl) might fail, the fix is to **control the
source** -- mirror the asset to one of our `*-v<N>` GitHub releases via the upstream-cache pattern above -- not
to wrap the call in a retry tool. We tried the retry route ([`github:dbohdan/recur`][recur]) and ripped it back out:

- The dominant failure mode for our fetches is **rate-limit** (GH
  asset-CDN 429, raw.githubusercontent.com 60-req/min, HuggingFace per-IP). Retry timing (a few minutes of exponential
  backoff) does not clear the reset window, so the chain just exhausts its attempts and fails anyway.
- The other failure modes recur could in principle help with --
  transient TCP resets, DNS hiccups, mid-transfer connection drops --
  are rare enough on GHA runners that **a single manual rerun is
  cheaper than the complexity** (a tool entry + a `[vars] retry`
  wrapper + an eager install in `_setup_all` + the `{{ vars.retry }}`
  prefix on every call site + a conftest rule enforcing the wrapper).
- Recur itself caused multiple Windows-specific pathologies during its
  brief life in this repo -- `cmd /s /c "mise exec -- recur ..."`
  subprocess setlocal recursion via mise.cmd shims, MSYS-bash cygheap
  fork-copy exhaustion under repeated retries, .cmd-extension PATH
  shim chains. Net contributor-time cost was substantially negative.

So: if you're staring at a flaky fetch, the right move is to add an `upstream-cache` entry that mirrors the asset
to our control, **not** to reintroduce a retry layer. The only exception worth considering is if a future
regression genuinely produces a high rate of transient-network failures (TCP-level, not rate-limit) on a fetch
we can't mirror -- at that point, the recur recipe is preserved in git history (search for `github:dbohdan/recur`
to find the prior implementation), and the right thing is to reintroduce it **at one specific call site only**,
not as a repo-wide var-prefix.

### Cargo HTTP/2 framing layer failures

One transient class that hits the `cargo fetch` path specifically: libcurl's `[16] Error in the HTTP2 framing layer`
during a `crates.io` download. Captured example:
`https://github.com/edge-toolkit/core/actions/runs/27900771461/job/82560594632`
on commit `6e4c0030a830e4f8e6b15381cb5c2fbf481af704` -- `asyncapi-rust-codegen` download bailed with `curl failed` ->
`[16] Error in the HTTP2 framing layer`.

`CARGO_NET_RETRY` does **not** cover this -- the framing error surfaces from inside libcurl as a generic
`curl failed`, and cargo's retry classifier doesn't recognise it as transient. The targeted fix is
`CARGO_HTTP_MULTIPLEXING=false`, which disables HTTP/2 multiplexing in cargo's libcurl and falls back to HTTP/1.1
connections (immune to the framing-stream desync at the cost of slightly fewer multiplexed requests). Set it at
the workflow `env:` (or `.mise/config.toml`'s `[env]`) so every cargo invocation in CI + local picks it up.

[recur]: https://github.com/dbohdan/recur

## taiki-e/install-action's resolution chain

The `.github/actions/install-mise` composite action uses
[`taiki-e/install-action`](https://github.com/taiki-e/install-action) to fetch the mise binary plus any tools
passed via its `extra-tools:` input. When deciding whether install-action can supply a given tool, walk its
three-tier chain:

1. **install-action's own TOOLS manifest** -- its
   [`TOOLS.md`](https://github.com/taiki-e/install-action/blob/main/TOOLS.md)
   is the authoritative list of tools it ships hand-curated prebuilt-URL
   manifests for. Fastest path; one HTTP fetch per tool.
2. **cargo-quickinstall fallback** -- if the tool name isn't in the manifest,
   install-action hands off to `cargo-binstall`, which in turn checks
   [cargo-quickinstall's release index](https://github.com/cargo-bins/cargo-quickinstall/releases)
   for prebuilt assets. This covers a much wider set of crates than the
   curated manifest (anything with enough downloads gets auto-published).
3. **Source build via `cargo install`** -- when neither of the above has a
   prebuilt for the target triple, `cargo-binstall` falls through to a real
   source build. Slow; treat its presence in CI as a regression.

When the question is "can install-action install X?", check the TOOLS.md first; if not there, search
cargo-quickinstall's release tags for `X-<ver>`. A hit in either tier means install-action will fetch a prebuilt;
absence in both means CI would pay a source-build cost.

Even a manifest hit isn't a guarantee -- install-action's resolver insists on the exact binary names its manifest
expects to find inside the prebuilt archive, so an upstream rename makes it fall through to a real `cargo install`
source build (which then flakes on crates.io) with no signal beyond a `bin <name> is not found` line. Reach for
install-action only when the prebuilt actually exists for the target triple _and_ the binary names still match.

## Linting

Lint checks must be expressed through one of the repo's linters -- **never** as a bespoke shell script, whether a
standalone file or a mise task `run`. The available linters:

- **ast-grep** (`config/ast-grep/rules/`) -- structural rules for code **and
  YAML** (e.g. GitHub Actions workflows).
- **semgrep** (`config/semgrep/`) -- incl. `languages: [generic]`, which works on
  TOML/text (e.g. `mise-config.yaml` lints `.mise/config*.toml`).
- **taplo** JSON-schemas (`config/taplo/`) -- TOML structure, applied via
  `taplo lint --schema` in `taplo-check`.
- **conftest** (`config/conftest/policy/`) -- Rego policies over the combined
  TOML/YAML config set, for cross-file checks the schema linters can't express.
- **regal** (`config/regal.yaml`, via `regal-check`) -- lints the Rego policies
  themselves (style, idioms, bugs). Carves out conftest-specific patterns
  (multiple `deny` rules per file, cross-package `data.*` references, no OPA
  entrypoints) that vanilla regal would flag.
- **shellcheck on mise task bodies** (`.mise/shellcheck-mise.jq`, via
  `shellcheck-mise-check`) -- extracts every multi-line bash-shell `run = """ ...
  """` from `.mise/config*.toml`, masks Tera tokens (`{{ ... }}` -> `MISEVAR`,
  `{% ... %}` -> empty), and shellchecks the lot. This is how shell-quality
  lints reach mise task bodies (shellcheck itself doesn't read TOML).
- plus hadolint, ls-lint (file/dir naming), zizmor (Actions security), ryl
  (YAML), lychee (links), clang-format / clang-tidy / cpplint / flawfinder (C, in the zig config),
  editorconfig-checker, typos, and action-validator for their domains.

ast-grep has no TOML grammar, so it **cannot** lint TOML -- use a taplo schema or
a semgrep `generic` rule there. If none of the above can express a check,
propose adding a new mise-installable linter rather than scripting it by hand.

### Maximise linter coverage; suppress only narrowly, with justification

This project aims for an extremely high DevSecOps bar, and the governing goal is **maximum linter rule coverage** --
across Rust, every guest language, the configs, Dockerfiles, GitHub Actions, and the external services (DeepSource,
Codacy, ...). The default is always to enable more rules and more linters, never fewer; a rule that catches a real
class of defect is worth the friction.

Because of that, **never turn a lint off to make code pass, and never avoid a security-related lint.** Do not disable
a rule, switch off a whole analyzer or rule class (e.g. an analyzer's `misra_compliance`-style toggle), exclude a
whole file or directory from analysis, or otherwise silence findings wholesale -- each trades real coverage for a
green check, and silencing a security lint is never acceptable.

When a finding is a genuine false positive or an unavoidable, reviewed-safe necessity, suppress it at the **narrowest
possible scope** -- one line, one rule -- with a justification stating _why_ it is safe or false. In order of
preference:

1. **Fix the code** so the finding no longer fires -- the only right answer for anything actionable.
2. **Inline, per-finding suppression carrying a reason** -- `#[expect(..., reason = "...")]`, `// skipcq: <code>`,
   `// flawfinder: ignore [why]`, a `nosemgrep: <rule>` note, etc. -- scoped to the single line and single rule,
   never a bare blanket ignore that also hides future (possibly real) findings on that line.
3. Only if neither works, a tightly-scoped path-or-rule carve-out in the linter config, commented with the exact
   reason -- and this needs operator sign-off (see the rule-deletion note below).

Excluding a directory or disabling a rule class is the last resort, not the first. Treat any new blanket exclusion or
disabled security lint in a diff as a red flag to push back on.

When step 3 is genuinely forced -- the analyzer offers no per-line or per-pattern suppression (some external services,
e.g. Codacy, only support path excludes) and the finding is a reviewed-safe necessity that cannot be fixed -- the
exclusion must still be as **narrow as the file layout allows**. Do not exclude a whole directory (or a file that also
holds analyzable code) to silence one un-suppressible construct: first refactor the code so the un-suppressible part is
**isolated into its own smallest-possible file**, then exclude only that file. Everything factored out of it stays
analyzed. The `services/ws-web-runner/mingw-shim/msvc_crt_alloc.c` split is the worked example -- the shim's only
MISRA-21.3-unavoidable heap-allocation code (operator new/delete + `_dupenv_s`) was pulled out of
`msvc_crt_shim.c` / `msvc_crt_locale.c` into that one file so Codacy's path exclude covers just it, leaving the rest of
the shim under full analysis (and even the excluded file stays covered by DeepSource's clang-tidy plus the repo's own
clang-tidy / cpplint / flawfinder). Record the exclusion rationale once, in the **excluded file's own header comment**
-- that is its canonical home, because more than one analyzer config may exclude the same file and the reasoning must
not be copied into each. Every config's exclude entry carries at most a one-line pointer back to the file, never a
second copy of the rationale.

### When you spot a style or consistency issue, write a rule

If a code-review comment, a fix-up commit, or a CLAUDE.md paragraph would tell the next contributor "don't do X"
or "always do Y", that's evidence the codebase wants a _rule_, not just a note. Reach for the linter stack first:
which of the available tools (`ast-grep` / `semgrep` / `taplo` / `conftest` / `regal` / `shellcheck-mise` / ...) can
express the rule? Code-as-policy stays in sync with the codebase; prose drifts. Documentation has its place when
the rule is fundamentally judgement-based (the "Workarounds" section's "embed the exact error message verbatim"
guidance, say) -- but if the check is mechanical, make it mechanical.

### When you write a rule, try to make it auto-fixable

ast-grep rules accept a `fix:` field; semgrep rules accept `fix:` / `fix-regex:`; clippy lints can be
machine-applicable; the repo's `*-fix` mise tasks (`ast-grep-fix`, `semgrep-fix`, `cargo-clippy-fix`, `ruff-fix`,
`regal-fix`, `typos-fix`, `oxlint-fix`, `clang-tidy-fix`, and the `fix` aggregator that runs them all under
`fix:rust` / `fix:<lang>` namespacing) apply those fixes in place. A check that takes one mechanical rewrite to
satisfy is much cheaper to land than one that requires a human edit per site, so the autofix scales the rule.
When the rewrite can't be expressed as a single template (multiple match shapes, context-dependent replacement,
structural restructuring), keep the rule check-only and write a brief note in the rule body explaining why the
autofix wasn't viable.

### Expand `doc-summary-ends-with-period` as you touch Rust files

`config/ast-grep/rules/doc-summary-ends-with-period.yaml` enforces that a doc comment's first line is a
one-line summary ending in terminal punctuation (`.`, `!`, or `?`). The workspace had a large pre-existing
backlog of violations, so the rule is scoped by a `files:` allowlist rather than applied workspace-wide, and it
is rolled out incrementally. **Whenever you modify a Rust file, add it to that rule's `files:` list (keep the
list sorted) and fix any first-line-summary violations in it as part of the same change** -- so the rule's
coverage only ever grows. Once the `files:` list covers effectively everything, drop the `files:` scoping and
let it apply workspace-wide.

### NEVER delete a lint rule without explicit user permission

**Do not delete a rule from `config/ast-grep/rules/`, `config/semgrep/`, `config/conftest/policy/`, `config/taplo/`,
`config/regal.yaml`, the `.editorconfig`, or any other linter ruleset without explicit operator sign-off, even
when the rule is "in the way" of a change you're making.** Rules exist because someone hit a specific regression
and codified the prevention. Deleting one removes the guardrail without removing the failure mode. If a rule is
blocking work:

- Prefer narrowing it (e.g. adding a name-prefix carve-out, a path exclude,
  or a `not:` constraint) over deleting it.
- If the rule has a genuine duplicate elsewhere (another linter that checks
  the same thing), removing one of the pair is a project-policy change and still needs the user to OK it explicitly --
  say which rule you want to drop and why, and wait.
- If the rule is genuinely obsolete (the bug it prevents no longer exists in
  the codebase, the language/tool it targeted is gone, etc.), propose the removal and wait for confirmation.

A previously-incorrect call here: deleting `config/ast-grep/rules/gha-no-step-shell.yaml` on the grounds that it
was "redundant with the conftest rule in gha.rego." Even though both rules checked the same thing, removing the
ast-grep one took out a diff-time safety net that catches the issue in IDE / pre-commit contexts where conftest
isn't always run. The right move was to add the same carve-out to both, not delete one.

## Writing JS/TS that both dprint and oxfmt accept

Both formatters run on every `*.js` / `*.ts` file. `config/dprint.jsonc`'s `typescript` block and
`config/oxfmtrc.jsonc` are tuned to agree on the structural choices each tool exposes as config knobs (arrow-paren
style, binary-operator position, member-chain breaking, line width). Two unconfigurable structural decisions
still trip them up -- both reduce to:

> **When an assignment statement exceeds `printWidth` (120), dprint and oxfmt
> break it in different places.**

dprint inserts a linebreak after `=`, keeping the RHS one connected unit. oxfmt prefers to keep the assignment
one-line and break inside the RHS, OR in template-literal cases, refuses to break the literal at all and overflows
silently. Either way they disagree, and there's no dprint/oxfmt knob to reconcile them.

The fix is in the source, not the config: **keep every assignment statement under 120 chars**. When the RHS is genuinely
long, extract intermediates.

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

If you really cannot get the line under 120 (rare in practice), the next escape is `// dprint-ignore` on that
single line -- but verify oxfmt also leaves it alone after dprint runs.

## Linter ignores: keep in sync with .gitignore

Some linters walk the working tree directly and never read `.gitignore`. When
you add a path to `.gitignore`, update their ignore lists too:

- **lychee** -- `config/lychee.toml`'s `exclude_path`. Its gitignore filter
  exists but doesn't cross pnpm's symlink farm under `node_modules/.pnpm/`.
- **ls-lint** -- `config/ls-lint.yaml`'s `ignore`. Patterns are gitignore-style
  globs; bare names match only top-level -- use `**/<name>` for nested matches
  (e.g. pnpm puts `node_modules` under each package dir, not the repo root).

A new linter that surfaces ignored paths in its output belongs on this list.

## Do NOT add gitignored paths to ec / dprint / typos config

`editorconfig-checker` (`ec`), `dprint`, and `typos` already honor `.gitignore` on their own. If a path is in
`.gitignore`, those three linters skip it -- full stop. Do **not** add a redundant exclude to `.editorconfig`'s
`[path/**]` blocks, `config/dprint.jsonc`'s `excludes`, or `config/typos.toml`'s `extend-exclude` "just to be
safe" or "to be explicit". A redundant exclude is a lie -- it implies the path needs special handling when in
fact `.gitignore` already covers it, and the next reader has to grep two places to understand the same fact.

The right move when one of these linters flags a generated / vendored / build- output tree:

1. Add the path to `.gitignore` (and run `mise run gen:dockerignore` if it
   also belongs in `.dockerignore`).
2. Stop. Do not touch `.editorconfig` / `dprint.jsonc` / `typos.toml`.

The narrow exception: a path that **must** stay tracked in git (so cannot go in `.gitignore`) but still needs the
linter to ignore it. `generated/` trees that we commit (`generated/python-rest/`, `generated/python-ws/`, etc.)
are the canonical case -- see the existing `[generated/python-rest/**]` block in `.editorconfig` for the shape.
Reach for a config-file exclude only after confirming gitignoring isn't viable.

## No `scripts/` directory

Do not create a `scripts/` directory or drop loose shell/Python scripts in the
repo. Every script belongs in one of two places:

- **Short and simple** -> an inline `mise` task (`run = """ ... """` in
  `.mise/config.toml` or a `.mise/config.<lang>.toml`). It stays discoverable
  via `mise tasks` and runs as `mise run <name>`.
- **More involved** -> its own tool directory under `utilities/` with its own
  `README.md` documenting what it does and how to run it.

## Don't depend on host tools in mise tasks

A mise task must never assume a command-line utility happens to exist on the host (or in a base image). Use the mise-
managed, version-pinned, cross-platform tool instead, so a task behaves identically on CI, in the Docker images, on a
workstation, and on every OS. Reach for a host binary only if there is genuinely no mise tool for it -- and then add
one rather than depending on the host.

What to use instead of the common host utilities (a list, not a table -- dprint pads table columns, which blows the
120-char limit):

- `cut`, `ls`, `sort`, `mktemp`, `cat`, ... -> `coreutils <util>` (uutils multicall;
  always invoke with the explicit `coreutils` prefix)
- `grep` -> `rg` (ripgrep)
- `find`, `xargs` -> bare `find` / `xargs` (uutils `findutils` mise tool; its shims
  shadow the host's)
- `awk` -> `goawk`
- `sed` -> no tool; rewrite the step with `coreutils`, `rg -r`, or `goawk`

Ripgrep is **recursive by default** -- there is no recursive flag to add, and `rg PATTERN` already walks the tree.
In particular, do NOT reach for `-r` thinking it means "recursive": in ripgrep `-r`/`--replace` takes the
replacement text as its value, so a bundled `-rn "PATTERN"` parses as `--replace=n` and silently rewrites every
match to `n` in the output -- a mangled result that looks like a real search. Use `rg -n "PATTERN"` for line numbers
(recursion is implicit); use `-r` only when you deliberately want substitution (as the `sed` row above does).

`coreutils`, `ripgrep`, `findutils` and `goawk` are mise `[tools]`. `coreutils` and `ripgrep` are additionally
force-installed by `_setup_all`, because the `preinstall` task itself uses them before the main `mise install` runs.

What the Dockerfiles `apt-get install` is therefore only genuine build prerequisites the toolchain needs (compilers,
libraries, the archive tools mise unpacks downloads with) -- never POSIX utilities, which now all come from tools.

One Nano Server exception: `Dockerfile.nanoserver` does not put mise's shims on `PATH` (native busybox-w32 can't
use the msys-form paths mise injects for POSIX shells -- see the `http:busybox` note in `config.windows.toml`),
so the Windows `preinstall` can't call these tools bare -- it goes through `mise exec --` or a shell builtin
instead. **TODO (next time we improve `Dockerfile.nanoserver`):** work out a busybox-compatible way to get the
shims (or tool bins) onto `PATH` so Windows tasks can call `coreutils`/`rg`/`goawk` directly like every other OS,
and drop the `mise exec --` / shell-builtin workarounds.

## Rust Workspace

Single Cargo workspace (`Cargo.toml`). Shared dependency versions are declared in `[workspace.dependencies]`. Add
new deps there, not in individual crate `[dependencies]`.

## Avoid GPL and LGPL dependencies

Every license `config/deny.toml`'s `[licenses] allow` list currently accepts is permissive (Apache-2.0, MIT,
BSD, Zlib, ISC, MPL-2.0, etc.) -- none copyleft. Keep it that way: do not add a new direct or transitive
dependency whose license is GPL (any version) or LGPL (any version), and do not add GPL/LGPL to the
`deny.toml` allow list, even when the license is individually OSI-approved (LGPL is). Being OSI-approved is
not sufficient on its own here -- copyleft obligations are a different category of concern from a permissive
license, and accepting one is a project-policy decision, not a mechanical license check.

Concrete precedent: `viuer` (added for the storage service's tty image-preview feature) pulled in
`ansi_colours` under LGPL-3.0-or-later as an unconditional dependency for its RGB-to-256-color quantization
path. Rather than allow-listing LGPL, the feature was reimplemented as a small hand-rolled ANSI truecolor
renderer in `services/storage/src/tty_image.rs`, built on the `image` crate the workspace already depended on
via `deno_image`/`ws-web-runner` -- truecolor output needs no palette quantization at all, so there was
nothing left for the LGPL dependency to do. If a future dependency's only viable path involves a GPL/LGPL
component, treat that the same way: look for a permissively-licensed alternative or a narrower reimplementation
first, and only bring the question to the user if neither is workable -- don't add the exception yourself.

## Clippy lints

**Never weaken or disable a lint to make code pass -- not the workspace lint config (`[workspace.lints.*]`,
`config/clippy.toml` thresholds, the ast-grep / taplo rules) -- without explicit operator permission.** Setting a
denied lint to `allow` or raising a threshold is a project-policy change, not a fix. If a lint is in the way, fix
the code (or justify it with a scoped `#[expect(..., reason = "...")]`); if you believe the lint itself is wrong,
stop and ask.

Clearing an `#[expect(...)]` that clippy reports as **unfulfilled** is normally fine and expected -- that's the
intended cleanup. **The one exception is `clippy::cognitive_complexity`** (and other macro-expansion-sensitive
lints): do **not** auto-remove those `#[expect]`s, because of the gotcha below.

**Feature-unification gotcha.** `clippy::cognitive_complexity` can fire in the full-workspace build but not in an
isolated `cargo clippy -p <crate>`, because `-p` enables fewer features (e.g. the `tracing` macros expand further
when the whole workspace's tracing/otel features are on). So its `#[expect]` can look _unfulfilled_ in an
isolated run yet be _required_ by CI. **Validate with the full `mise run check`, not an isolated `-p` clippy,
before touching one of these `#[expect]`s.** (The clean fix -- `[resolver] feature-unification = "workspace"` so
every build sees the same features -- is still nightly-only via `-Z feature-unification`.)

The workspace denies a broad set of clippy lints (see `[workspace.lints.clippy]` in `Cargo.toml`), including
restriction lints. One you'll hit often: **`clippy::single_call_fn`** fires on a private function called from
exactly one site. Do **not** inline the function just to silence it -- a function that is a distinct, named step
(kept separate for readability, or that will gain more callers) is legitimate. Keep it and annotate with
`#[expect(...)]` and a real justification:

```rust
#[expect(clippy::single_call_fn, reason = "distinct step of X; kept separate for readability and future reuse")]
fn helper(...) { ... }
```

Use `#[expect(...)]` rather than `#[allow(...)]` (the workspace denies `unfulfilled_lint_expectations`, so an
`expect` that stops applying fails the build instead of silently lingering). The same applies to other
restriction lints whose pattern is intentional in a given spot -- prefer a justified
`#[expect(..., reason = "...")]` over contorting the code to dodge the lint.

## Naming conventions

- **`.map_err` wrappers must be named `map_*`.** Extension methods that
  hide a `.map_err(...)` call (e.g. converting a foreign error to a
  domain error type) must keep `map_` in the name. The reader can then
  tell at the call site that this is a _mapping_ over the error, not
  some unrelated boolean predicate. Example: the `JsErrExt` trait in
  `services/ws-web-runner/src/error.rs` exposes `.map_js_err()` and
  `.map_js_err_with_context(...)`, never `.js_err()` / `.to_js_err()` /
  similar.
