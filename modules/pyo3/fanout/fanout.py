# noqa: INP001 -- et-ws-pyo3-runner compiles this one file on its own; it is not a package
"""Emit multiple outbound frames per inbound frame via the `WsSender` push API.

Used by `services/ws-pyo3-runner/tests/modules.rs` to verify the multi-send path works end to end.

For each inbound binary frame containing `n` (a single byte 0-255), we
push `n` distinct binary frames back through `send.binary(...)`. We
return `None` so reply-by-return doesn't add an extra frame.
"""

from __future__ import annotations

import logging
from typing import Any

_logger = logging.getLogger(__name__)

_state: dict[str, Any] = {}  # "send": the WsSender, set in init()


def init(send, _storage) -> None:
    """Stash the WsSender for the fan-out path."""
    _state["send"] = send
    # `_storage` ignored -- fanout doesn't persist anything.
    _logger.info("fanout agent initialised")


def on_binary_frame(frame: bytes) -> None:
    """Push one binary frame per unit of the count in the first byte."""
    if not frame:
        return
    count = frame[0]
    for i in range(count):
        _state["send"].binary(bytes([i]))
    return
