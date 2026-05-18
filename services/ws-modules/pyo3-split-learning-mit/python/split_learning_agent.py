"""Split-learning server-side agent for `et-ws-pyo3-runner`.

Port of the MIT split-learning-demo's `scripts/server.py` to the generic
pyo3-runner contract:

    init(send)
    on_connect(agent_id)
    reply = on_binary_frame(frame)   # per inbound binary frame
    on_shutdown()

The model serves request/response (one inbound activations frame → one
outbound grads / logits frame), so `on_binary_frame` uses reply-by-
return for the actual training/inference response. The `send` argument
is stashed in `_send` for completeness; a future enhancement (e.g.
periodic loss telemetry, a "training_started" announcement) would push
through it independently.

The wire format (base64-encoded JSON envelope with base64-encoded tensor
blobs inside `raw`) is decoded and encoded here in Python — the Rust
runner only shuttles raw bytes. State (model, optimizer, fabric handle)
lives in module-level globals, mirroring the singleton-server model the
demo's `server.py` already assumed.

Configuration via environment variables (all optional):

  SPLIT_LEARNING_ONNX_PATH   — where to load/save server weights (ONNX).
                               If the file exists it's loaded on init;
                               if training happened, it's written on
                               shutdown.
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

import logging
import os
from pathlib import Path

import lightning as L
import onnx
import torch
from onnx import numpy_helper
from torch import nn

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


# --- module state ----------------------------------------------------------
# `init()` populates these; every handler reads them. There's exactly one
# split-learning server per process (it owns the weights file), so a
# module-level singleton is the natural fit.

_agent_id: str | None = None
_send = None  # type: WsSender | None — kept for future push-style telemetry
_fabric: L.Fabric | None = None
_model: nn.Module | None = None
_unwrapped_model: nn.Module | None = None
_optimizer: torch.optim.Optimizer | None = None
_criterion: nn.Module | None = None
_onnx_path: Path | None = None
_trained: bool = False


# --- weight load/save — straight port of helpers in scripts/server.py ------


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


# --- runner hooks ----------------------------------------------------------


def init(send) -> None:
    """Build the server-side split-learning model.

    Reads config from env. `_unwrapped_model` is preserved so
    `on_shutdown` can export the untouched module — `fabric.setup` wraps
    it and would otherwise leak the wrapper into the saved ONNX graph.
    """
    global _send, _fabric, _model, _unwrapped_model, _optimizer, _criterion, _onnx_path
    _send = send

    learning_rate = float(os.environ.get("SPLIT_LEARNING_LEARNING_RATE", "1e-4"))
    accelerator = os.environ.get("SPLIT_LEARNING_ACCELERATOR", "auto")
    onnx_path_env = os.environ.get("SPLIT_LEARNING_ONNX_PATH")
    _onnx_path = Path(onnx_path_env) if onnx_path_env else None

    _fabric = L.Fabric(accelerator=accelerator, precision="32-true")
    _fabric.launch()

    backbone = CNN2D(in_channels=1, dim_out=10, img_size=28, dropout=0.15)
    model = CNN2DServer(in_channels=1, dim_out=10, img_size=28, model=backbone)

    if _onnx_path is not None and _onnx_path.exists():
        unmatched = _load_onnx_weights(model, _onnx_path)
        if unmatched:
            _logger.warning("ONNX weights not loaded for: %s", unmatched)
        else:
            _logger.info("Loaded server weights from %s", _onnx_path)
    elif _onnx_path is not None:
        _logger.info("No server weights at %s; starting from random", _onnx_path)
    else:
        _logger.info("SPLIT_LEARNING_ONNX_PATH not set; starting from random; weights won't be saved")

    _unwrapped_model = model
    optimizer = torch.optim.SGD(model.parameters(), lr=learning_rate, momentum=0.9)
    _criterion = nn.CrossEntropyLoss()
    _model, _optimizer = _fabric.setup(model, optimizer)


def on_connect(agent_id: str) -> None:
    global _agent_id
    _agent_id = agent_id
    _logger.info("split-learning agent registered as %s", agent_id)


def on_binary_frame(frame: bytes) -> bytes | None:
    """Process one binary frame from the demo client.

    Frames are base64(utf8(json(WSMessage))) — same envelope server.py
    reads. Returns the encoded response frame, or None for messages we
    don't act on.
    """
    try:
        message = decode_message_b64(frame)
    except Exception as e:
        _logger.warning("frame decode failed: %s", e)
        return None

    if message.type == MessageType.ACTIVATIONS_AND_LABELS:
        return _step_training(message)
    if message.type == MessageType.ACTIVATIONS:
        return _step_inference(message)

    _logger.debug("ignoring frame of type %s", message.type)
    return None


def on_shutdown() -> None:
    """Persist trained weights on disconnect — mirrors server.py's
    WebSocketDisconnect handler."""
    if not _trained:
        _logger.info("no training happened; skipping ONNX export")
        return
    if _onnx_path is None:
        _logger.warning("trained but SPLIT_LEARNING_ONNX_PATH unset; weights discarded")
        return
    assert _fabric is not None and _unwrapped_model is not None
    example_input = torch.zeros(1, 16, 7, 7, device=_fabric.device)
    _export_onnx(_unwrapped_model, _onnx_path, example_input)
    _logger.info("saved trained server weights to %s", _onnx_path)


# --- per-message step logic — lifted from server.py ------------------------


def _step_training(message: WSMessage) -> bytes:
    global _trained
    assert _fabric is not None and _model is not None and _optimizer is not None and _criterion is not None

    activations = deserialize_tensor(message.raw["tensor"], dtype=torch.float32)
    labels = deserialize_tensor(message.raw["labels"], dtype=torch.int64)

    activations = activations.to(_fabric.device)
    activations = activations.reshape(*message.data["tensor_shape"])
    labels = labels.to(_fabric.device)

    _optimizer.zero_grad()
    _model.train()
    _trained = True
    activations.requires_grad = True
    outputs = _model(activations)
    loss = _criterion(outputs, labels)
    _fabric.backward(loss)
    _optimizer.step()

    grads = activations.grad
    client_grads = grads.detach().clone()
    serialized = serialize_tensor(client_grads.cpu())
    response = WSMessage(
        type=MessageType.GRADS,
        data={"tensor_shape": list(grads.shape), "loss": loss.item()},
        raw={"tensor": serialized},
    )
    return encode_message_b64(response)


def _step_inference(message: WSMessage) -> bytes:
    assert _fabric is not None and _model is not None

    activations = deserialize_tensor(message.raw["tensor"], dtype=torch.float32)
    activations = activations.to(_fabric.device)
    activations = activations.reshape(*message.data["tensor_shape"])

    _model.eval()
    with torch.no_grad():
        outputs = _model(activations)

    logits = outputs.detach().clone()
    serialized = serialize_tensor(logits.cpu())
    response = WSMessage(
        type=MessageType.LOGITS,
        data={"tensor_shape": list(logits.shape)},
        raw={"tensor": serialized},
    )
    return encode_message_b64(response)
