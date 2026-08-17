"""FedAvg math1 twin for `et-ws-pyo3-runner`: storage-driven, on native CPython.

A fake agent injects the canonical input JSON (client datasets + hyperparameters) into ws-server
storage and broadcasts a `math1-input` pointer, which arrives here as an unrecognised text frame.
This module reads the input through the runner's storage handle, runs the FedAvg kernel -- only
+ - * / on floats, so the result is bit-identical to the other math1 twins -- and stores the global
model to math1-output.json in its own bucket, where the test harness reads and verifies it.
"""

from __future__ import annotations

import json
import logging
from typing import Any

_logger = logging.getLogger(__name__)

_storage: Any = None


def fed_avg(clients: list, rounds: int, epochs: int, learning_rate: float) -> tuple[float, float]:
    """Run the FedAvg simulation and return the final global (weight, bias)."""
    weight = 0.0
    bias = 0.0
    total_samples = 0.0
    for samples in clients:
        total_samples += float(len(samples))
    for _ in range(rounds):
        merged_weight = 0.0
        merged_bias = 0.0
        for samples in clients:
            count = float(len(samples))
            client_weight = weight
            client_bias = bias
            for _ in range(epochs):
                grad_weight = 0.0
                grad_bias = 0.0
                for sample in samples:
                    residual = client_weight * sample[0] + client_bias - sample[1]
                    grad_weight += residual * sample[0]
                    grad_bias += residual
                client_weight -= learning_rate * (2.0 * grad_weight / count)
                client_bias -= learning_rate * (2.0 * grad_bias / count)
            merged_weight += client_weight * count
            merged_bias += client_bias * count
        weight = merged_weight / total_samples
        bias = merged_bias / total_samples
    return weight, bias


def init(_send, storage) -> None:
    """Stash the WsStorage handle for the exchange."""
    global _storage
    _storage = storage


def on_text_frame(text: str) -> None:
    """On the math1-input pointer broadcast: read the input, compute, and store the output.

    The pointer is re-broadcast until the harness sees the output, so duplicates just recompute
    and re-store the same bytes -- idempotent by construction.
    """
    try:
        msg = json.loads(text)
    except ValueError:
        return
    if not (isinstance(msg, dict) and msg.get("type") == "math1-input"):
        return
    input_bytes = _storage.get(msg["bucket"], msg["filename"])
    if input_bytes is None:
        raise RuntimeError(f"input {msg['filename']} not found in bucket {msg['bucket']}")
    params = json.loads(bytes(input_bytes).decode("utf-8"))
    _logger.info(
        "running FedAvg - %d clients x %d rounds x %d local epochs",
        len(params["clients"]),
        params["rounds"],
        params["epochs"],
    )
    weight, bias = fed_avg(params["clients"], params["rounds"], params["epochs"], params["learning_rate"])
    _logger.info("global model weight=%r bias=%r", weight, bias)
    output = json.dumps({"module": "math1", "weight": weight, "bias": bias})
    _storage.put("math1-output.json", output.encode("utf-8"))
