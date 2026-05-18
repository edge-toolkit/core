"""In-process Python adapter for the split-learning server-side model.

The Rust agent (`et-ws-pyo3-split-learning-mit`) embeds CPython via PyO3 and
drives this module. Everything that touches PyTorch lives here so the Rust
side never has to know the shape of a `nn.Module`. The wire-level base64 /
JSON envelope is handled in Rust before calling in.

Lifecycle, mirroring `split-learning-demo/scripts/server.py`:
  - `init_state(onnx_path)`: build the CNN2DServer, load weights if any.
  - `process_activations_and_labels(...)`: training step — returns grads
    for the client's activations plus the scalar loss.
  - `process_activations(...)`: inference step — returns logits.
  - `export_state(state, onnx_path)`: persist trained weights to ONNX.

The module deliberately uses module-level functions rather than a Rust-owned
PyO3 class to keep the FFI surface small.
"""

from __future__ import annotations

import logging
from pathlib import Path
from typing import Any

import lightning as L
import numpy as np
import onnx
import torch
from onnx import numpy_helper
from torch import nn

from split_learning.models.vision.cnn_2d import CNN2D, CNN2DServer

_logger = logging.getLogger(__name__)


def _load_onnx_weights(model: nn.Module, onnx_path: Path) -> list[str]:
    onnx_model = onnx.load(str(onnx_path))
    onnx_weights = {
        init.name: torch.from_numpy(numpy_helper.to_array(init).copy())
        for init in onnx_model.graph.initializer
    }
    state_dict = model.state_dict()
    unmatched: list[str] = []
    for key, current in state_dict.items():
        candidate = onnx_weights.get(key)
        if candidate is not None and candidate.shape == current.shape:
            state_dict[key] = candidate
        else:
            unmatched.append(key)
    model.load_state_dict(state_dict)
    return unmatched


def init_state(
    onnx_path: str | None,
    learning_rate: float = 1e-4,
    accelerator: str = "auto",
) -> dict[str, Any]:
    """Set up Lightning Fabric, the server-side model, optimizer, and criterion.

    Returns a state dict the Rust side passes back on every call. The
    `unwrapped_model` reference is preserved so we can export weights on
    shutdown — fabric.setup wraps the module and would otherwise leak the
    wrapper into the saved graph.
    """
    fabric = L.Fabric(accelerator=accelerator, precision="32-true")
    fabric.launch()

    backbone = CNN2D(in_channels=1, dim_out=10, img_size=28, dropout=0.15)
    model = CNN2DServer(in_channels=1, dim_out=10, img_size=28, model=backbone)

    if onnx_path is not None:
        path = Path(onnx_path)
        if path.exists():
            unmatched = _load_onnx_weights(model, path)
            if unmatched:
                _logger.warning("ONNX weights not loaded for: %s", unmatched)
            else:
                _logger.info("Loaded server weights from %s", path)
        else:
            _logger.info("No server weights at %s; starting from random", path)

    unwrapped_model = model
    optimizer = torch.optim.SGD(model.parameters(), lr=learning_rate, momentum=0.9)
    criterion = nn.CrossEntropyLoss()
    model, optimizer = fabric.setup(model, optimizer)

    return {
        "fabric": fabric,
        "model": model,
        "unwrapped_model": unwrapped_model,
        "optimizer": optimizer,
        "criterion": criterion,
        "trained": False,
    }


def _deserialize_float32(data: bytes, shape: list[int]) -> torch.Tensor:
    arr = np.frombuffer(data, dtype=np.float32)
    # The wire format from the demo client passes raw bytes; the encoded
    # tensor is always contiguous so frombuffer + reshape is sufficient.
    # `.copy()` is needed because numpy frombuffer returns a read-only view
    # and torch.from_numpy on it produces a non-writable tensor that can't
    # carry gradients.
    return torch.from_numpy(arr.copy()).reshape(*shape)


def _deserialize_int64(data: bytes) -> torch.Tensor:
    arr = np.frombuffer(data, dtype=np.int64)
    return torch.from_numpy(arr.copy())


def _serialize(tensor: torch.Tensor) -> bytes:
    return np.ascontiguousarray(tensor.detach().cpu().numpy()).tobytes()


def process_activations_and_labels(
    state: dict[str, Any],
    activation_bytes: bytes,
    label_bytes: bytes,
    tensor_shape: list[int],
) -> dict[str, Any]:
    """One training step: forward + backward + optimizer step on the server side.

    Returns the per-batch loss, the gradients flowing back to the client
    (serialised + shape) so the client can finish its own backward pass.
    """
    fabric = state["fabric"]
    model = state["model"]
    optimizer = state["optimizer"]
    criterion = state["criterion"]

    optimizer.zero_grad()
    activations = _deserialize_float32(activation_bytes, tensor_shape).to(fabric.device)
    labels = _deserialize_int64(label_bytes).to(fabric.device)

    model.train()
    activations.requires_grad = True
    outputs = model(activations)
    loss = criterion(outputs, labels)
    fabric.backward(loss)
    optimizer.step()

    state["trained"] = True

    grads = activations.grad
    return {
        "tensor": _serialize(grads),
        "tensor_shape": list(grads.shape),
        "loss": float(loss.item()),
    }


def process_activations(
    state: dict[str, Any],
    activation_bytes: bytes,
    tensor_shape: list[int],
) -> dict[str, Any]:
    """Inference: forward pass returning logits, no gradient bookkeeping."""
    fabric = state["fabric"]
    model = state["model"]

    activations = _deserialize_float32(activation_bytes, tensor_shape).to(fabric.device)

    model.eval()
    with torch.no_grad():
        outputs = model(activations)

    return {
        "tensor": _serialize(outputs),
        "tensor_shape": list(outputs.shape),
    }


def export_state(state: dict[str, Any], onnx_path: str) -> None:
    """Export the server-side model to ONNX.

    The example input shape (1, 16, 7, 7) mirrors the demo's cut layer: the
    client sends post-conv1 activations (16 channels, 7x7 after the pool
    cascade on 28x28 MNIST), so that's the input the server-side graph
    expects.
    """
    if not state["trained"]:
        _logger.info("No training happened in this session; skipping ONNX export")
        return

    model = state["unwrapped_model"]
    fabric = state["fabric"]
    out_path = Path(onnx_path)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    example_input = torch.zeros(1, 16, 7, 7, device=fabric.device)
    was_training = model.training
    model.eval()
    try:
        torch.onnx.export(
            model,
            example_input,
            str(out_path),
            input_names=["input"],
            output_names=["output"],
            dynamic_axes={"input": {0: "batch"}, "output": {0: "batch"}},
            # See comment in scripts/server.py: the dynamo path trips an
            # onnxscript typeinfo check on Python 3.14, keep the legacy
            # TorchScript exporter explicit.
            dynamo=False,
        )
    finally:
        model.train(was_training)
    _logger.info("Saved trained server weights to %s", out_path)
