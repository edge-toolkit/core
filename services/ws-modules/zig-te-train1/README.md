# zig-te-train1

Zig WASM module that wraps a TinyEngine-generated training/inference path for the workspace server. Implements MCUNetV3 sparse-update training (arXiv:2206.15472) across three backbones: MCUNet, MobileNetV2, ProxylessNAS.

## Quick start (fresh checkout)

```bash
mise run setup
```

Runs the whole pipeline end-to-end: clones upstream repos, pulls the runtime vendor subset, installs the platform Python env, regenerates triplets, runs codegen, builds the wasm artifacts. Linux uses the locked Pixi env. Native Apple Silicon macOS builds TVM locally from source under `.tvm-macos/`. Takes a few minutes on Linux and substantially longer on first macOS setup; idempotent on re-runs.

Prereqs: `mise` and `zig` >= 0.16 on PATH. The module `.mise.toml` installs Pixi.

## Directories

- `src/` — Zig entrypoint + C bridge code that exposes the TinyEngine path
- `shim/` — CMSIS/ARM-DSP stubs so TinyEngine kernels compile cleanly for wasm
- `patches/` — Wrapper-style overrides for upstream tinyengine + tiny-training. No file under `vendor/upstream/` is ever modified (see `patches/README.md`)
- `tools/` — Helpers invoked by the mise tasks: `extract_snapshot.py` (emits `te_snapshot.c`), `helpers_prelude.c` + `helpers_block.c` (injected into `genModel.c`), `smoke_pathc.mjs` (headless verifier). The orchestration logic itself lives in `.mise.toml` as inline tasks.
- `codegen-{mcunet,mbv2,proxyless}/` — Regenerable C source for each backbone (output of `mise run codegen`)
- `triplets/{mcunet,mbv2,proxyless}/` — Regenerable IR triplets per backbone (`graph.json` + `params.pkl` + `scale.json`, output of `mise run triplets:regen`). Gitignored.
- `pkg/` — Web package artifacts: `pkg/demo/` (demo HTML/JS), `pkg/*.js`, `pkg/*.wasm` (the latter gitignored)
- `vendor/upstream/` — Full clones of upstream repos (gitignored, populated by `mise run setup`)
- `vendor/{tinyengine,cmsis}/` — Runtime kernel subset for the Zig build (output of `mise run pull-vendor`)

## Pipeline at a glance

```
upstream tiny-training            ──▶ Stage 1 ──▶ triplets/MODEL/
  (pretrained .pkl checkpoints       (mise run triplets:regen)
   in vendor/upstream/.../assets/    needs locked Pixi env
   mcu_models/)                      (apache-tvm + torch)

triplets/MODEL/                   ──▶ Stage 2 ──▶ codegen-MODEL/
                                     (mise run codegen)
                                     uses locked Pixi env

codegen-MODEL/ + src/             ──▶ zig build ──▶ pkg/*.wasm
                                     (mise run build)
```

## Common tasks

| Task                              | What it does                                                                  |
| --------------------------------- | ----------------------------------------------------------------------------- |
| `mise run setup`                  | End-to-end from a fresh checkout (slow, ~3-5 min)                             |
| `mise run setup:upstream`         | Clone upstream tinyengine + tiny-training (idempotent)                        |
| `mise run pull-vendor`            | Refresh `vendor/{tinyengine,cmsis}/` runtime kernels from the pinned snapshot |
| `mise run env:create`             | Install the platform Python env for regeneration                              |
| `mise run triplets:regen [MODEL]` | Regen triplets (Stage 1); needs the platform Python env. `MODEL` defaults to `all` |
| `mise run codegen [MODEL]`        | Regen codegen-MODEL/ from triplets (Stage 2); needs the platform Python env   |
| `mise run build`                  | `zig build -Doptimize=ReleaseSmall`                                           |
| `mise run setup:sample-pack`      | Populate the Demo's gitignored COCO val2014 sample pack                       |
| `mise run smoke`                  | Smoke-verify mcunet wasm: pool max Δ, binary margins, sparse update fires     |

## Demo sample pack

The browser demo can train from generated synthetic samples, uploaded images, or
a prepared local sample pack. To use the demo's **Load sample pack** button, run
this once from `services/ws-modules/zig-te-train1`:

```bash
mise run setup:sample-pack
```

This creates the gitignored directory `pkg/demo/sample-pack/`. The button loads
`pkg/demo/sample-pack/manifest.json` plus the listed images from the same demo
server. If that directory has not been prepared, **Load sample pack** reports
that the sample pack is unavailable.

`setup:sample-pack` downloads COCO 2014 validation annotations and only the
selected images from `images.cocodataset.org`, samples 40 person + 40 scene
training images and 10 person + 10 scene validation images, cover-crops them to
128x128, and writes the local pack.

Useful variants:

- Use a different sampling seed:

  ```bash
  mise run setup:sample-pack 123
  ```

- Discard the cache and redownload annotations/images:

  ```bash
  mise run setup:sample-pack --force-download
  ```

The full `mise run setup` pipeline does not build the sample pack by default.
To include it in a fresh setup run, use:

```bash
WITH_SAMPLE_PACK=1 mise run setup
```

## Python Env

`env:create` is platform-aware:

- **linux-64**: installs the locked Pixi env from `pixi.toml` / `pixi.lock`.
- **osx-arm64**: uses `tools/tvm-macos-env/pixi.toml` for build tools, builds
  TVM 0.11.1 from source into the gitignored `.tvm-macos/` directory, and
  creates `.tvm-macos/venv`.

The macOS path intentionally does not use Homebrew. It still requires Apple's
compiler toolchain from Xcode Command Line Tools:

```bash
xcode-select --install
```

The macOS TVM source build enables LLVM because `triplets:regen` needs
`target.build.llvm` during Relay/autodiff. The Pixi build-tool env pins
`llvmdev` to 14.x to avoid TVM 0.11.1 compile failures against newer LLVM APIs.
If a previous no-LLVM `.tvm-macos/` build exists, `mise run env:create` rebuilds
it automatically when the recorded build config does not match.

The regeneration stages then use:

- **Stage 1** (`triplets:regen`): python 3.9 + apache-tvm + torch CPU.
- **Stage 2** (`codegen`): runs the TinyEngine codegen wrapper from the same platform env.

Set `PYTHON` to use a specific interpreter directly. On Linux, if Pixi is not
on PATH, the tasks can still fall back to conda via `CONDA_ENV`.

## Patches: how upstream behavior is changed without source edits

See `patches/README.md` for details. Briefly: every upstream-modifying patch is a separate wrapper file under `patches/{tinyengine,tiny-training}/` that monkey-patches the upstream class or function at import time via Python's runtime binding. The upstream tree under `vendor/upstream/` is never written to, so fresh clones produce identical results to ours.
