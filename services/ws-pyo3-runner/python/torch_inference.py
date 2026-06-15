"""PyTorch analogue of the wasi-graphics-info module, for `et-ws-pyo3-runner`.

Where wasi-graphics-info runs a deterministic 4x4 matmul (verifying C[0][0]) and
a single MNIST forward pass (verifying the predicted class) through standardised
WASI interfaces, this runs the same two shapes through PyTorch on the embedded
CPython interpreter:

  1. compute:   C = A @ B with A = I(4), B = 2*I(4); verify C[0][0] == 2.0
  2. inference: a fixed tiny linear classifier over a constant input; argmax ->
                class; verify it matches the deterministic expected class

`torch` is declared as `pipx:torch` in the python-only mise config and reaches
`sys.path` via `edge_toolkit::config::mise_python_site_packages`, exactly like
cowsay. The top-level `import torch` fails the whole module load if torch isn't
wired in -- so `tests/torch_inference.rs` checks for torch up front and SKIPS,
rather than letting the runner fail, when torch isn't installed.

On any inbound text frame the module runs the workflow and returns a JSON
summary, so the Rust test can round-trip a trigger and assert on the result.
"""

from __future__ import annotations

import json

import torch

# Identity * (2*I); mirrors MAT_A / MAT_B / EXPECTED_C00 in wasi-graphics-info.
EXPECTED_C00 = 2.0
# Fixed weights make the classifier's argmax deterministic across builds.
EXPECTED_CLASS = 3


def _matmul() -> float:
    """C = I(4) @ (2 * I(4)); the (0,0) cell is 2.0."""
    a = torch.eye(4)
    b = torch.eye(4) * 2.0
    c = a @ b
    return float(c[0, 0].item())


def _inference() -> int:
    """Run a fixed 1x4 input through a fixed 4x4 weight matrix.

    The last row dominates, so argmax is deterministically EXPECTED_CLASS. No
    randomness, no model file -- the point is to exercise a real torch forward
    pass.
    """
    x = torch.tensor([[1.0, 2.0, 3.0, 4.0]])
    weights = torch.tensor(
        [
            [0.0, 0.0, 0.0, 0.0],
            [0.1, 0.0, 0.0, 0.0],
            [0.0, 0.1, 0.0, 0.0],
            [1.0, 1.0, 1.0, 1.0],
        ]
    )
    logits = x @ weights.T
    return int(torch.argmax(logits, dim=1).item())


def on_text_frame(text: str) -> str:
    """Run both checks and return a JSON summary.

    Raise on any mismatch so a regression surfaces as a failed module rather
    than a wrong-but-quiet reply.
    """
    c00 = _matmul()
    if abs(c00 - EXPECTED_C00) > 1e-4:
        raise RuntimeError(f"matmul C[0][0]={c00}, expected {EXPECTED_C00}")
    predicted = _inference()
    if predicted != EXPECTED_CLASS:
        raise RuntimeError(f"inference predicted {predicted}, expected {EXPECTED_CLASS}")
    return json.dumps(
        {
            "framework": "torch",
            "torch_version": torch.__version__,
            "matmul_c00": c00,
            "predicted_class": predicted,
            "expected_class": EXPECTED_CLASS,
        }
    )
