"""Module that exercises `WsStorage.get` / `WsStorage.put`.

On the first inbound binary frame, the module reads `key` (the first
bytes of the payload up to a NUL byte) and the rest of the payload as
the value, then calls `storage.put(key, value)`. On the second inbound
binary frame containing just `key`, it calls `storage.get(my_agent_id,
key)` and pushes the resulting bytes back via `send.binary(...)`.

Used by `tests/storage.rs` to verify the storage path lands bytes on
the ws-server and reads them back.
"""

from __future__ import annotations

import logging

_logger = logging.getLogger(__name__)

_send = None
_storage = None
_agent_id: str | None = None


def init(send, storage) -> None:
    global _send, _storage
    _send = send
    _storage = storage


def on_connect(agent_id: str) -> None:
    global _agent_id
    _agent_id = agent_id


def on_binary_frame(frame: bytes) -> None:
    """Frames are `key\\x00value` for puts, or `key` (no NUL) for gets."""
    if b"\x00" in frame:
        key_bytes, value = frame.split(b"\x00", 1)
        key = key_bytes.decode("utf-8")
        _storage.put(key, value)
        _logger.info("stored %d bytes at key=%s", len(value), key)
        return None

    key = frame.decode("utf-8")
    value = _storage.get(_agent_id, key)
    if value is None:
        _send.binary(b"")
    else:
        _send.binary(value)
    _logger.info("fetched key=%s (%d bytes)", key, 0 if value is None else len(value))
    return None
