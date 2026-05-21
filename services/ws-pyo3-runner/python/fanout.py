"""Example module that emits *multiple* outbound frames per inbound frame
using the `WsSender` push API. Used by `tests/fanout.rs` to verify the
multi-send path works end to end.

For each inbound binary frame containing `n` (a single byte 0-255), we
push `n` distinct binary frames back through `send.binary(...)`. We
return `None` so reply-by-return doesn't add an extra frame.
"""

from __future__ import annotations

import logging

_logger = logging.getLogger(__name__)

_send = None  # WsSender, set in init()


def init(send, storage) -> None:
    global _send
    _send = send
    # `storage` ignored — fanout doesn't persist anything.
    _logger.info("fanout agent initialised")


def on_binary_frame(frame: bytes) -> None:
    if not frame:
        return None
    count = frame[0]
    for i in range(count):
        _send.binary(bytes([i]))
    return None
