#!/usr/bin/env python3
"""
Stage 1 wrapper: regenerate the (graph.json + params.pkl + scale.json) triplet
for a chosen backbone, without modifying any file in vendor/upstream/tiny-training/.

Usage:
    python stage1_wrapper.py <model_name> <output_dir>

Where:
    <model_name> ∈ {mcunet, mbv2, proxyless}
    <output_dir> = where the three triplet files get written (e.g. triplets/mcunet/)

Reads paths from env vars:
    TINY_TRAINING_DIR — path to the cloned tiny-training repo

The wrapper does five things, all without touching upstream files:
  1. Sets sys.path so patches/tiny-training/ (this dir) is preferred over
     upstream — that lets `import mcu_ops_shim` resolve our shim file.
  2. Imports mcu_ops_shim, which registers nn.mcuconv2d / mcuadd / mcutruncate /
     mcumean on stock apache-tvm. Replaces the unbuildable TVM-hack fork.
  3. Pre-imports compilation.autodiff.diff_ops AND compilation.autodiff.int8_grad
     so both fire their @register_gradient side effects. Then restores
     GRAD_OP_MAP['nn.mcuconv2d'] to the diff_ops (fp32) version — the upstream
     code path imports int8_grad inside auto_diff's visit_call which clobbers
     the gradient for *all* mcuconv2d ops, including non-sparse ones; that's
     the bug we'd otherwise hit. We also replace nn.mcutruncate's gradient
     with our a_min/a_max-aware version because the upstream code reads
     orig.attrs.min/max (a TVM-hack-only field).
  4. Reads upstream mcu_ir_gen.py from disk into a string, substitutes the
     `model_name = "..."` assignment with the requested model, and exec()s
     the modified string. The on-disk file is never written to.
  5. Reads upstream ir2json.py, overrides sys.argv to point at the right IR,
     exec()s it. Output appears in .model/testproj/.

  Then copies the three files to <output_dir>/.
"""

import os
import re
import shutil
import sys
from pathlib import Path

# Smallest sparse_bp budget that tinyengine's codegen accepts for each model.
# (Smaller budgets have weight_update_ratio=0.125 which produces unaligned
# first_k_channel and trips a NotImplementedError downstream in tinyengine.)
MODEL_BUDGETS = {
    "mcunet": "49kb",
    "mbv2": "123kb",
    "proxyless": "74kb",
}


def _our_mcutruncate_grad(orig, grad):
    """Replacement for tiny-training's mcutruncate gradient. Reads from
    ClipAttrs.a_min / a_max (stock TVM) instead of TruncateAttrs.min / max
    (which only exists in the TVM-hack fork). Without this the autodiff
    fails with `AttributeError: 'NoneType' object has no attribute 'min'`."""
    from tvm import relay

    new_inputs = [relay.cast(a, "float32") for a in orig.args]
    x = new_inputs[0]
    dtype = "float32"
    a_min = getattr(orig.attrs, "a_min", -128.0) if orig.attrs is not None else -128.0
    a_max = getattr(orig.attrs, "a_max", 127.0) if orig.attrs is not None else 127.0
    lo = relay.const(float(a_min), dtype=dtype)
    hi = relay.const(float(a_max), dtype=dtype)
    mask = relay.greater_equal(x, lo) * relay.less_equal(x, hi)
    return [relay.where(mask, grad, relay.zeros_like(grad))]


def main():
    if len(sys.argv) != 3:
        sys.stderr.write(f"usage: {sys.argv[0]} <model_name> <output_dir>\n")
        return 2
    model_name, output_dir = sys.argv[1], sys.argv[2]
    if model_name not in MODEL_BUDGETS:
        sys.stderr.write(f"unknown model: {model_name} (expected one of {sorted(MODEL_BUDGETS)})\n")
        return 2
    budget = MODEL_BUDGETS[model_name]

    tty_dir = os.environ.get("TINY_TRAINING_DIR")
    if not tty_dir or not os.path.isdir(tty_dir):
        sys.stderr.write(
            "TINY_TRAINING_DIR env var must point at an existing directory (the cloned tiny-training repo).\n"
        )
        return 2
    tty_dir = os.path.abspath(tty_dir)
    patches_dir = str(Path(__file__).resolve().parent)

    # Step 1: sys.path — patches first (for mcu_ops_shim), then upstream.
    # Upstream's compilation/autodiff/{diff_ops,int8_grad}.py have several
    # function-local `from autodiff.diff_ops import ...` statements that
    # assume the interpreter is run from inside compilation/. Adding that
    # dir to sys.path lets those bare imports resolve. (Triggered by mbv2 +
    # proxyless's sparse_depth_wise_mcunetconv2d_grad path.)
    sys.path.insert(0, tty_dir)
    sys.path.insert(0, os.path.join(tty_dir, "compilation"))
    sys.path.insert(0, patches_dir)

    # Step 2: register custom TVM ops via the shim.
    import mcu_ops_shim  # noqa: F401

    # Step 3: ensure GRAD_OP_MAP entries for nn.mcuconv2d / nn.mcuadd point at
    # the fp32 versions (the int8 versions in int8_grad.py would otherwise
    # clobber them when auto_diff.py's visit_call lazily imports int8_grad).
    #
    # Subtle bug worth documenting: in upstream diff_ops.py, BOTH the
    # nn.mcuconv2d gradient (line ~163) AND the nn.mcuadd gradient (line ~820)
    # are named `mcunetconv2d_grad`. The second def overwrites the first in
    # the module namespace, so `diff_ops.mcunetconv2d_grad` resolves to the
    # mcuadd gradient (8 inputs), not the mcuconv2d gradient (6 inputs).
    # The decorator side-effects on GRAD_OP_MAP are correct though — we read
    # from there instead of from the module attribute.
    from compilation.autodiff import diff_ops  # noqa: F401  — registers fp32 grads
    from compilation.autodiff.op2grad import GRAD_OP_MAP

    saved_mcuconv2d_grad = GRAD_OP_MAP["nn.mcuconv2d"]
    saved_mcuadd_grad = GRAD_OP_MAP["nn.mcuadd"]

    # Pre-import int8_grad to fire its @register_gradient side effects at a
    # predictable point (here, rather than lazily inside visit_call). After
    # this, the module is in sys.modules and the lazy import is a no-op.
    from compilation.autodiff import int8_grad  # noqa: F401  — clobbers ↑↑

    # Restore the fp32 versions for mcuconv2d + mcuadd, and install our
    # custom mcutruncate grad (both diff_ops and int8_grad read attrs.min/max
    # which is a TVM-hack-only attribute; stock TVM uses a_min/a_max).
    GRAD_OP_MAP["nn.mcuconv2d"] = saved_mcuconv2d_grad
    GRAD_OP_MAP["nn.mcuadd"] = saved_mcuadd_grad
    GRAD_OP_MAP["nn.mcutruncate"] = _our_mcutruncate_grad

    # Step 4: load + exec mcu_ir_gen.py with model_name override.
    mcu_ir_gen_path = os.path.join(tty_dir, "compilation/mcu_ir_gen.py")
    src = Path(mcu_ir_gen_path).read_text()
    src = re.sub(r'^model_name\s*=\s*"[^"]*"', f'model_name = "{model_name}"', src, flags=re.MULTILINE, count=1)
    saved_cwd = os.getcwd()
    os.chdir(tty_dir)
    try:
        exec(compile(src, mcu_ir_gen_path, "exec"), {"__name__": "__main__", "__file__": mcu_ir_gen_path})
    finally:
        os.chdir(saved_cwd)

    # Step 5: load + exec ir2json.py against the just-generated sparse_bp IR.
    ir2json_path = os.path.join(tty_dir, "compilation/ir2json.py")
    src = Path(ir2json_path).read_text()
    rel_ir = f"ir_zoos/{model_name}_quantize/sparse_bp-{budget}-1x3x128x128.ir"
    saved_argv = sys.argv[:]
    sys.argv = [ir2json_path, rel_ir]
    saved_cwd = os.getcwd()
    os.chdir(tty_dir)
    try:
        exec(compile(src, ir2json_path, "exec"), {"__name__": "__main__", "__file__": ir2json_path})
    finally:
        sys.argv = saved_argv
        os.chdir(saved_cwd)

    # Copy outputs into <output_dir>.
    output_dir = os.path.abspath(output_dir)
    os.makedirs(output_dir, exist_ok=True)
    base = f"sparse_bp-{budget}-1x3x128x128"
    shutil.copy(os.path.join(tty_dir, ".model/testproj", f"{base}-graph.json"), os.path.join(output_dir, "graph.json"))
    shutil.copy(os.path.join(tty_dir, ".model/testproj", f"{base}-params.pkl"), os.path.join(output_dir, "params.pkl"))
    shutil.copy(
        os.path.join(tty_dir, f"ir_zoos/{model_name}_quantize/scale.json"), os.path.join(output_dir, "scale.json")
    )
    sys.stderr.write(f"✓ wrote triplet for {model_name} (sparse_bp-{budget}) to {output_dir}/\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
