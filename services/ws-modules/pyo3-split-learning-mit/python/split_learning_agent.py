"""Split-learning server-side agent for `et-ws-pyo3-runner`.

Port of the MIT split-learning-demo's `scripts/server.py` to the generic
pyo3-runner contract:

    init(send, storage)
    on_connect(agent_id)              # load weights via storage.get
    reply = on_binary_frame(frame)    # per inbound binary frame
    on_shutdown()                     # persist weights via storage.put

Persistence runs through et-ws-server's `/storage` HTTP API rather than
the local filesystem. The Python module asks the storage handle for our
weights blob on connect and uploads the trained blob on shutdown. A
temp file is still used as an intermediary because torch's ONNX exporter
and `onnx.load` only accept paths — but the durable copy lives on the
ws-server, scoped under the agent's own `/storage/<agent_id>/` prefix.

Configuration via environment variables (all optional):

  SPLIT_LEARNING_WEIGHTS_KEY      — storage key for the weights blob,
                                    default `server_mnist.onnx`. Lives
                                    under `/storage/<our-agent-id>/<key>`.
  SPLIT_LEARNING_SOURCE_AGENT_ID  — if set, load initial weights from
                                    this agent's storage namespace
                                    instead of our own. Useful for
                                    bootstrapping a new agent_id from
                                    weights an earlier run uploaded.
  SPLIT_LEARNING_LEARNING_RATE    — SGD learning rate, default 1e-4
  SPLIT_LEARNING_ACCELERATOR      — Lightning Fabric accelerator backend
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
import tempfile
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

_agent_id: str | None = None
_send = None  # type: WsSender | None — kept for future push-style telemetry
_storage = None  # type: WsStorage | None — set in init()
_fabric: L.Fabric | None = None
_model: nn.Module | None = None
_unwrapped_model: nn.Module | None = None
_optimizer: torch.optim.Optimizer | None = None
_criterion: nn.Module | None = None
_weights_key: str = "server_mnist.onnx"
_source_agent_id: str | None = None
_trained: bool = False


# --- ONNX helpers — same logic as scripts/server.py, but path-based --------


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


def init(send, storage) -> None:
    """Build the (untrained) server-side model. Weight loading happens
    in `on_connect` because `storage.put`/`storage.get` need an agent_id
    that isn't known until et-connect-ack."""
    global _send, _storage, _fabric, _model, _unwrapped_model, _optimizer, _criterion
    global _weights_key, _source_agent_id
    _send = send
    _storage = storage

    learning_rate = float(os.environ.get("SPLIT_LEARNING_LEARNING_RATE", "1e-4"))
    accelerator = os.environ.get("SPLIT_LEARNING_ACCELERATOR", "auto")
    _weights_key = os.environ.get("SPLIT_LEARNING_WEIGHTS_KEY", "server_mnist.onnx")
    _source_agent_id = os.environ.get("SPLIT_LEARNING_SOURCE_AGENT_ID") or None

    _fabric = L.Fabric(accelerator=accelerator, precision="32-true")
    _fabric.launch()

    backbone = CNN2D(in_channels=1, dim_out=10, img_size=28, dropout=0.15)
    model = CNN2DServer(in_channels=1, dim_out=10, img_size=28, model=backbone)

    _unwrapped_model = model
    optimizer = torch.optim.SGD(model.parameters(), lr=learning_rate, momentum=0.9)
    _criterion = nn.CrossEntropyLoss()
    _model, _optimizer = _fabric.setup(model, optimizer)


def on_connect(agent_id: str) -> None:
    """Look up cached weights in /storage and load them if present."""
    global _agent_id
    _agent_id = agent_id
    _logger.info("split-learning agent registered as %s", agent_id)

    assert _storage is not None and _unwrapped_model is not None
    source_agent = _source_agent_id or agent_id
    blob = _storage.get(source_agent, _weights_key)
    if blob is None:
        _logger.info(
            "no cached weights at /storage/%s/%s; starting from random",
            source_agent,
            _weights_key,
        )
        return

    # onnx.load only takes paths — buffer to a temp file, then drop it.
    with tempfile.NamedTemporaryFile(suffix=".onnx", delete=False) as tmp:
        tmp.write(blob)
        tmp_path = Path(tmp.name)
    try:
        unmatched = _load_onnx_weights(_unwrapped_model, tmp_path)
        if unmatched:
            _logger.warning("ONNX weights not loaded for: %s", unmatched)
        else:
            _logger.info(
                "loaded %d-byte server weights from /storage/%s/%s",
                len(blob),
                source_agent,
                _weights_key,
            )
    finally:
        tmp_path.unlink(missing_ok=True)


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
    """Persist trained weights via storage.put — mirrors server.py's
    WebSocketDisconnect handler, but the durable copy lives on the
    ws-server instead of the runner's local disk."""
    if not _trained:
        _logger.info("no training happened; skipping ONNX upload")
        return
    if _storage is None or _unwrapped_model is None or _fabric is None:
        _logger.warning("trained but runner not fully initialised; weights discarded")
        return

    # torch.onnx.export wants a path. We dump into a temp file, then
    # stream the bytes up to /storage and drop the file.
    with tempfile.NamedTemporaryFile(suffix=".onnx", delete=False) as tmp:
        tmp_path = Path(tmp.name)
    try:
        example_input = torch.zeros(1, 16, 7, 7, device=_fabric.device)
        _export_onnx(_unwrapped_model, tmp_path, example_input)
        blob = tmp_path.read_bytes()
        _storage.put(_weights_key, blob)
        _logger.info(
            "uploaded %d-byte trained weights to /storage/%s/%s",
            len(blob),
            _storage.agent_id,
            _weights_key,
        )
    finally:
        tmp_path.unlink(missing_ok=True)


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
