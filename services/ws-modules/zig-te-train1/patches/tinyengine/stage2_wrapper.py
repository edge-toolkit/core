#!/usr/bin/env python3
"""
Stage 2 wrapper: runs tinyengine's codegen against a triplet, with our
group_conv2d patch applied via in-memory method replacement. Upstream files
under vendor/upstream/tinyengine/ are never modified.

Usage:
    python stage2_wrapper.py <graph.json> <params.pkl> <scale.json>

The wrapper:
  1. Adds vendor/upstream/tinyengine/ to sys.path so `code_generator.*`
     resolves to the upstream tree.
  2. Adds this directory (patches/tinyengine/) to sys.path so
     `import group_conv2d_patch` resolves to our wrapper.
  3. Imports `code_generator.operators.group_conv2d` (loads upstream class).
  4. Imports `group_conv2d_patch` (replaces generate_inference_str via
     class attribute assignment — side effect of the import).
  5. Builds sys.argv to match upstream's `examples/tiny_training.py` CLI
     and runpy-runs it.

Reads env vars:
    TINYENGINE_DIR — path to the cloned tinyengine repo.
"""

import os
import runpy
import sys
from pathlib import Path


def main():
    if len(sys.argv) != 4:
        sys.stderr.write(f"usage: {sys.argv[0]} <graph.json> <params.pkl> <scale.json>\n")
        return 2

    graph_path, params_path, scale_path = (os.path.abspath(p) for p in sys.argv[1:4])

    tt_dir = os.environ.get("TINYENGINE_DIR")
    if not tt_dir or not os.path.isdir(tt_dir):
        sys.stderr.write("TINYENGINE_DIR env var must point at an existing directory (the cloned tinyengine repo).\n")
        return 2
    tt_dir = os.path.abspath(tt_dir)
    patches_dir = str(Path(__file__).resolve().parent)

    # Step 1+2: sys.path — upstream first (so its code_generator.* resolves),
    # then patches (so group_conv2d_patch is findable).
    sys.path.insert(0, tt_dir)
    sys.path.insert(0, patches_dir)

    # Step 3+4: import upstream class, then our patch (which monkey-patches
    # `generate_inference_str` on the class via side effect at import time).
    import code_generator.operators.group_conv2d  # noqa: F401 — load class first
    import group_conv2d_patch  # noqa: F401 — replaces method on the class

    # Step 5: drive examples/tiny_training.py via runpy. We replicate the CLI
    # form we always use manually:
    #   python examples/tiny_training.py -f <graph> -D <params> -QAS <scale> -g -FR
    entry_path = os.path.join(tt_dir, "examples/tiny_training.py")
    saved_argv = sys.argv[:]
    sys.argv = [
        entry_path,
        "-f",
        graph_path,
        "-D",
        params_path,
        "-QAS",
        scale_path,
        "-g",
        "-FR",
    ]
    saved_cwd = os.getcwd()
    os.chdir(tt_dir)
    try:
        runpy.run_path(entry_path, run_name="__main__")
    finally:
        sys.argv = saved_argv
        os.chdir(saved_cwd)
    return 0


if __name__ == "__main__":
    sys.exit(main())
