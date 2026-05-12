# patches/

Wrapper-style patches that adapt stock upstream sources without modifying any
file under `vendor/upstream/`. Every change to upstream behavior is implemented
as a separate file in this directory.

## Why wrappers and not source edits

`vendor/upstream/tinyengine/` and `vendor/upstream/tiny-training/` are full
clones of the upstream repos. They are pulled by `mise run setup`. The
codegen automation here is reproducible only as long as those clones stay
_pristine_ — if we patched them in place, a fresh checkout would produce
different output than ours did. Wrappers achieve the same behavior change
through Python's runtime monkey-patching, leaving the upstream tree untouched.

## What's in here

### `tiny-training/mcu_ops_shim.py`

Registers four custom TVM ops on stock `apache-tvm`:

- `nn.mcuconv2d` — MCUNetV3's int8 sparse-update conv
- `nn.mcuadd` — int8 residual add
- `nn.mcutruncate` — clip to a_min/a_max
- `mcumean` — global average pool

Upstream tiny-training requires a custom _TVM fork_ with these ops baked in.
The shim replaces that fork requirement by registering the ops at module-load
time via `tvm.relay.op.op.register_op_attr`. Required because building the
TVM fork is impractical.

### `tiny-training/stage1_wrapper.py`

Orchestrates Stage 1 (IR + triplet generation) without modifying upstream
files. Three things this wrapper navigates around in upstream:

1. **`mcu_ops_shim` ordering.** Sets up `sys.path` with `patches/` before
   `vendor/upstream/tiny-training/` so the shim resolves first.

2. **Gradient name-shadowing bug.** Upstream `diff_ops.py` has _two_
   functions named `mcunetconv2d_grad` — the line-163 one registered for
   `nn.mcuconv2d` (6 inputs), and the line-820 one registered for
   `nn.mcuadd` (8 inputs). The second `def` shadows the first in the module
   namespace, so `diff_ops.mcunetconv2d_grad` resolves to the _mcuadd_
   gradient. We read both correct entries out of `GRAD_OP_MAP` (the
   decorator-populated registry), then pre-import `int8_grad` (which would
   otherwise clobber lazily during `visit_call`), then restore.

3. **`mcutruncate` attrs.** Both upstream gradients read
   `orig.attrs.min/max` which only exists on the TVM-hack fork's
   `TruncateAttrs`. Stock TVM's `ClipAttrs` uses `a_min/a_max`. We install
   our own `_our_mcutruncate_grad` to read the stock names.

4. **Bare-import resolution.** A few functions deep in upstream
   (`diff_ops.py:283/544/683` and `int8_grad.py:294/458`) have
   `from autodiff.diff_ops import ...` — bare imports that assume the
   interpreter was started from inside `compilation/`. We add
   `vendor/upstream/tiny-training/compilation/` to `sys.path` so these
   resolve.

After that, reads `compilation/mcu_ir_gen.py` into a string, substitutes
the `model_name` literal via regex, and `exec()`s it. Same for
`compilation/ir2json.py`. The on-disk files are never rewritten.

Invoked by the `triplets:regen` mise task — the rare-path orchestrator.

### `tinyengine/group_conv2d_patch.py`

Replaces `code_generator.operators.group_conv2d.groupConv2d.generate_inference_str`
with a copy of the upstream method body plus one extra `elif` in the inplace
check:

```python
elif params["output_c"] == params["input_c"] == params["groups"]:
    function_name += "_inplace"
```

Without this branch, upstream raises `NotImplementedError` when the
sparse-update gradient lands on a depthwise-equivalent conv (groups == in ==
out, ratio = 1) — which happens for mbv2 and proxyless. The depthwise_conv_fp
kernel template already exists for this shape; the upstream check just forgot
to allow the `_inplace` suffix on it.

The wrapper imports the upstream class, defines a `_patched_generate_inference_str`
function (~230 lines, a faithful copy of the upstream method body with the
one-line patch), then assigns it back as a class attribute. Side-effect at
import time — no source edit.

### `tinyengine/stage2_wrapper.py`

Orchestrates Stage 2 (codegen from a triplet). Adds the upstream tree to
`sys.path`, imports `code_generator.operators.group_conv2d` (loads the class),
imports `group_conv2d_patch` (the import side effect monkey-patches the
class), then `runpy.run_path()`s `examples/tiny_training.py` with the CLI
args `-f <graph> -D <params> -QAS <scale> -g -FR`.

Invoked by the `codegen` mise task — the main orchestrator.

## How the orchestrators wire it up

The orchestration is inline in `.mise.toml`. The wrappers below are invoked by those tasks.

```
  mise run setup
     └─ git clone tinyengine    → vendor/upstream/tinyengine
        git clone tiny-training → vendor/upstream/tiny-training

  mise run codegen [MODEL]      (common path; MODEL defaults to "all")
     ├─ cp triplets/MODEL/* → tinyengine/assets/
     ├─ python patches/tinyengine/stage2_wrapper.py …
     │     └─ in-memory monkey-patches group_conv2d, then
     │        runpy's tinyengine/examples/tiny_training.py
     ├─ cp tinyengine/codegen/ → codegen-MODEL/
     ├─ python tools/extract_snapshot.py MODEL   (writes te_snapshot.c)
     ├─ awk-inject tools/helpers_prelude.c after the #includes in genModel.c
     └─ cat tools/helpers_block.c >> codegen-MODEL/source/genModel.c

  mise run triplets:regen [MODEL]    (rare path; linux-64 wheel or osx-arm64 local TVM build)
     └─ python patches/tiny-training/stage1_wrapper.py MODEL triplets/MODEL/
           ├─ registers mcu_ops_shim onto stock TVM
           ├─ fixes GRAD_OP_MAP entries (mcuconv2d/mcuadd/mcutruncate)
           ├─ exec()s upstream mcu_ir_gen.py
           └─ exec()s upstream ir2json.py
```

The orchestrators never write into `vendor/upstream/`. They only consume
files from it (or copy _into_ `vendor/upstream/tinyengine/assets/`, which is
the codegen entry point that already accepts file inputs).

## Two-stage design

**Stage 1** (`triplets:regen`) is the heavy path: it uses python 3.9 +
apache-tvm + the tiny-training model checkpoints and runs the autograd
pipeline. Linux uses the locked Pixi env. Native Apple Silicon macOS uses the
gitignored `.tvm-macos/` source build prepared by `mise run env:create`. It
produces a `triplet = (graph.json, params.pkl, scale.json)` per model. The
triplets are committed under `triplets/MODEL/` so most devs never need to run
Stage 1.

**Stage 2** (`codegen`) takes a committed triplet and emits the C source. It
uses the same platform Python env by default and is fast (~5 seconds per
model). This is the path devs use to rebuild after pulling new triplets or
after changes to the wrappers/orchestrators.

## Adding a new patch

1. Drop the wrapper file into the appropriate sub-directory:
   - `tiny-training/` for changes that affect Stage 1
   - `tinyengine/` for changes that affect Stage 2
2. If the wrapper monkey-patches a class/function, make sure the relevant
   `stage{1,2}_wrapper.py` imports the upstream symbol _first_ and then
   imports your patch — the import-time side effect of your patch replaces
   the binding.
3. Document the _why_ in the wrapper's docstring (what was the upstream bug
   or limitation? what's the minimal change?). Future devs need to be able
   to reproduce the reasoning if upstream's source shifts.

## What's NOT in here

- The runtime kernel sources used by the Zig build live in
  `vendor/tinyengine/` and `vendor/cmsis/`. Those are pulled by
  `mise run pull-vendor` (a separate task, separate purpose).
- Per-model TinyEngine-side fixes that survive into the runtime build (e.g.
  the LogSoftmax `FLT_MIN` typo) get patched against the runtime sources by
  the Zig build itself — see `build.zig` for those.
