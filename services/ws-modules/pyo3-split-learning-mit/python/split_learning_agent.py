"""Split-learning server-side agent for `et-ws-pyo3-runner`.

This is a port of the MIT split-learning-demo's `scripts/server.py` to the
generic pyo3-runner contract:

    state = init()
    set_agent_id(state, agent_id)
    reply = handle_binary(state, frame)   # per inbound binary frame
    shutdown(state)

The wire format (base64-encoded JSON envelope with base64-encoded tensor
blobs inside `raw`) is decoded and encoded here in Python — the Rust
runner only shuttles raw bytes. That keeps the runner agnostic and the
demo protocol self-contained on the Python side.

Configuration via environment variables (all optional):

  SPLIT_LEARNING_ONNX_PATH   — where to load/save server weights (ONNX).
                               If the file exists it's loaded on init; if
                               training happened, it's written on shutdown.
  SPLIT_LEARNING_LEARNING_RATE
                             — SGD learning rate, default 1e-4
  SPLIT_LEARNING_ACCELERATOR — Lightning Fabric accelerator backend
                               (auto|cpu|gpu|cuda|mps|tpu), default auto

To run end-to-end against et-ws-server:

  PYO3_AGENT_MODULE=split_learning_agent \\
  PYO3_AGENT_PYTHONPATH=services/ws-modules/pyo3-split-learning-mit/python:\\
                       split-learning-demo/packages/split-learning-demo/src \\
  WS_SERVER_URL=ws://127.0.0.1:8080/ws \\
  cargo run -p et-ws-pyo3-runner
"""

from __future__ import annotations

import base64
import json
import logging
import os
from pathlib import Path
from typing import Any

import lightning as L
import onnx
import torch
from onnx import numpy_helper
from torch import nn

import numpy as np

# split_learning lives in the upstream demo's package — caller is expected
# to put its `src/` on PYO3_AGENT_PYTHONPATH so this import resolves.
from split_learning.models.vision.cnn_2d import CNN2D, CNN2DServer
from split_learning.schemas.message import MessageType, WSMessage
from split_learning.utils.serde import (
    decode_message_b64,
    deserialize_tensor,
    encode_message_b64,
    serialize_tensor,
)

_logger = logging.getLogger(__name__)


# ---------------------------------------------------------------------------
# Weight load/save — straight port of the helpers in scripts/server.py.

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


def _export_onnx(model: nn.Module, onnx_path: Path, example_input: torch.Tensor) -> None:
    onnx_path.parent.mkdir(parents=True, exist_ok=True)
    was_training = model.training
    model.eval()
    try:
        torch.onnx.export(
            model,
            example_input,
            str(onnx_path),
            input_names=["input"],
            output_names=["output"],
            dynamic_axes={"input": {0: "batch"}, "output": {0: "batch"}},
            # Force the legacy TorchScript exporter: the dynamo path pulls in
            # onnxscript's torchlib registry, which on Python 3.14 trips a
            # `typing.Union` typeinfo check in onnxscript 0.5.6.dev*.
            dynamo=False,
        )
    finally:
        model.train(was_training)


# ---------------------------------------------------------------------------
# Runner contract.

def init() -> dict[str, Any]:
    """Build the server-side split-learning model.

    Returns the state dict carried through the rest of the lifecycle. The
    `unwrapped_model` reference is preserved so `shutdown` can export the
    untouched module — `fabric.setup` would otherwise wrap it and leak the
    wrapper into the ONNX graph.
    """
    learning_rate = float(os.environ.get("SPLIT_LEARNING_LEARNING_RATE", "1e-4"))
    accelerator = os.environ.get("SPLIT_LEARNING_ACCELERATOR", "auto")
    onnx_path_env = os.environ.get("SPLIT_LEARNING_ONNX_PATH")
    onnx_path = Path(onnx_path_env) if onnx_path_env else None

    fabric = L.Fabric(accelerator=accelerator, precision="32-true")
    fabric.launch()

    backbone = CNN2D(in_channels=1, dim_out=10, img_size=28, dropout=0.15)
    model = CNN2DServer(in_channels=1, dim_out=10, img_size=28, model=backbone)

    if onnx_path is not None and onnx_path.exists():
        unmatched = _load_onnx_weights(model, onnx_path)
        if unmatched:
            _logger.warning("ONNX weights not loaded for: %s", unmatched)
        else:
            _logger.info("Loaded server weights from %s", onnx_path)
    elif onnx_path is not None:
        _logger.info("No server weights at %s; starting from random", onnx_path)
    else:
        _logger.info("SPLIT_LEARNING_ONNX_PATH not set; starting from random; weights won't be saved")

    unwrapped_model = model
    optimizer = torch.optim.SGD(model.parameters(), lr=learning_rate, momentum=0.9)
    criterion = nn.CrossEntropyLoss()
    model, optimizer = fabric.setup(model, optimizer)

    return {
        "agent_id": None,
        "fabric": fabric,
        "model": model,
        "unwrapped_model": unwrapped_model,
        "optimizer": optimizer,
        "criterion": criterion,
        "onnx_path": onnx_path,
        "trained": False,
    }


def set_agent_id(state: dict[str, Any], agent_id: str) -> None:
    state["agent_id"] = agent_id
    _logger.info("split-learning agent registered as %s", agent_id)


def handle_binary(state: dict[str, Any], frame: bytes) -> bytes | None:
    """Process one binary frame from the demo client.

    Frames are base64(utf8(json(WSMessage))) — same envelope server.py reads.
    Returns the encoded response frame, or None for messages we don't act on.
    """
    try:
        message = decode_message_b64(frame)
    except Exception as e:
        _logger.warning("frame decode failed: %s", e)
        return None

    if message.type == MessageType.ACTIVATIONS_AND_LABELS:
        return _step_training(state, message)
    if message.type == MessageType.ACTIVATIONS:
        return _step_inference(state, message)

    _logger.debug("ignoring frame of type %s", message.type)
    return None


def shutdown(state: dict[str, Any]) -> None:
    """Persist trained weights on disconnect — mirrors server.py's
    WebSocketDisconnect handler."""
    if not state.get("trained"):
        _logger.info("no training happened; skipping ONNX export")
        return
    onnx_path = state.get("onnx_path")
    if onnx_path is None:
        _logger.warning("trained but SPLIT_LEARNING_ONNX_PATH unset; weights discarded")
        return
    fabric = state["fabric"]
    model = state["unwrapped_model"]
    example_input = torch.zeros(1, 16, 7, 7, device=fabric.device)
    _export_onnx(model, onnx_path, example_input)
    _logger.info("saved trained server weights to %s", onnx_path)


# ---------------------------------------------------------------------------
# Per-message step logic. Lifted from server.py but rewritten as pure
# functions over `state` so we don't carry a global mutable session.

def _step_training(state: dict[str, Any], message: WSMessage) -> bytes:
    fabric = state["fabric"]
    model = state["model"]
    optimizer = state["optimizer"]
    criterion = state["criterion"]

    activations = deserialize_tensor(message.raw["tensor"], dtype=torch.float32)
    labels = deserialize_tensor(message.raw["labels"], dtype=torch.int64)

    activations = activations.to(fabric.device)
    activations = activations.reshape(*message.data["tensor_shape"])
    labels = labels.to(fabric.device)

    optimizer.zero_grad()
    model.train()
    state["trained"] = True
    activations.requires_grad = True
    outputs = model(activations)
    loss = criterion(outputs, labels)
    fabric.backward(loss)
    optimizer.step()

    grads = activations.grad
    client_grads = grads.detach().clone()
    serialized = serialize_tensor(client_grads.cpu())
    response = WSMessage(
        type=MessageType.GRADS,
        data={"tensor_shape": list(grads.shape), "loss": loss.item()},
        raw={"tensor": serialized},
    )
    return encode_message_b64(response)


def _step_inference(state: dict[str, Any], message: WSMessage) -> bytes:
    fabric = state["fabric"]
    model = state["model"]

    activations = deserialize_tensor(message.raw["tensor"], dtype=torch.float32)
    activations = activations.to(fabric.device)
    activations = activations.reshape(*message.data["tensor_shape"])

    model.eval()
    with torch.no_grad():
        outputs = model(activations)

    logits = outputs.detach().clone()
    serialized = serialize_tensor(logits.cpu())
    response = WSMessage(
        type=MessageType.LOGITS,
        data={"tensor_shape": list(logits.shape)},
        raw={"tensor": serialized},
    )
    return encode_message_b64(response)


# Silence unused-import warnings if static analysers ever scan this file;
# `base64`, `json`, `np` are deliberately kept as they document the
# transport assumptions even though the demo's serde helpers cover the
# actual codec.
_unused = (base64, json, np)
