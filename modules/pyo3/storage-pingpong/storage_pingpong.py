# noqa: INP001 -- et-ws-pyo3-runner compiles this one file on its own; it is not a package
"""Module that exercises `WsStorage.get` / `WsStorage.put`.

On the first inbound binary frame, the module reads `key` (the first
bytes of the payload up to a NUL byte) and the rest of the payload as
the value, then calls `storage.put(key, value)`. On the second inbound
binary frame containing just `key`, it calls `storage.get(my_agent_id,
key)` and pushes the resulting bytes back via `send.binary(...)`.

Used by `services/ws-pyo3-runner/tests/modules.rs` to verify the storage
path lands bytes on the ws-server and reads them back.
"""

from __future__ import annotations

import logging
from typing import Any

_logger = logging.getLogger(__name__)

_state: dict[str, Any] = {}  # "send", "storage" and "agent_id", from init() and on_connect()


def init(send, storage) -> None:
    """Stash the WsSender and WsStorage handles."""
    _state["send"] = send
    _state["storage"] = storage


def on_connect(agent_id: str) -> None:
    """Record the assigned agent id for later get/put calls."""
    _state["agent_id"] = agent_id


def on_binary_frame(frame: bytes) -> None:
    r"""Put on `key\x00value` frames, get (and reply) on bare `key` frames."""
    if b"\x00" in frame:
        key_bytes, value = frame.split(b"\x00", 1)
        key = key_bytes.decode("utf-8")
        _state["storage"].put(key, value)
        _logger.info("stored %d bytes at key=%s", len(value), key)
        return

    key = frame.decode("utf-8")
    value = _state["storage"].get(_state["agent_id"], key)
    if value is None:
        _state["send"].binary(b"")
    else:
        _state["send"].binary(value)
    _logger.info("fetched key=%s (%d bytes)", key, 0 if value is None else len(value))
    return
