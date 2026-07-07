"""Example Python module for `et-ws-pyo3-runner`.

This file demonstrates the contract the runner expects. Every function is
optional -- if your module doesn't define it, the runner skips that hook.

Lifecycle, in order:

  init(send, storage)              # once, at startup; `send`/`storage` are host handles
  on_connect(agent_id)             # once, after et-connect-ack
  on_text_frame(text)              # per inbound text frame the et hub didn't recognise as a typed et-* message
  on_binary_frame(frame)           # per inbound binary frame
  on_shutdown()                    # once, after the websocket closes

Two ways to emit outbound frames:

* **Simple case (this file uses it):** return `str` from `on_text_frame`
  or `bytes` from `on_binary_frame`. The runner sends that single frame
  back. `return None` for silence.

* **Fan-out case:** call `send.text(...)` / `send.binary(...)` any
  number of times during a handler -- or later from a background thread.
  Both styles compose: anything you `send.*()` during a handler goes
  out *before* the value you `return`, in submission order.

State lives in module-level globals. The runner instantiates one copy
per process, so this is the same as a singleton -- no classes, no state
threading across the FFI boundary.

To use this module, set these env vars and run the runner:

  WS_SERVER_URL=ws://127.0.0.1:8080/ws
  RUNNER_MODULE=echo
  PYO3_PYTHONPATH=services/ws-pyo3-runner/python
  cargo run -p et-ws-pyo3-runner
"""

from __future__ import annotations

import logging
from typing import Any

_logger = logging.getLogger(__name__)

# --- module state ----------------------------------------------------------

_agent_id: str | None = None
_send: Any = None  # WsSender | None -- stashed for fan-out, unused here
_storage: Any = None  # WsStorage | None -- stashed for completeness
_echoed: int = 0


# --- runner hooks ----------------------------------------------------------


def init(send, storage) -> None:
    """Stash the WsSender and WsStorage handles for later use.

    Even modules that only use reply-by-return should accept and keep `send`
    -- it's how you'd push frames later (e.g. from a background thread).
    `storage` is the ws-server's `/storage` API; this example doesn't use it.
    """
    global _send, _storage
    _send = send
    _storage = storage
    _logger.info("echo agent initialised")


def on_connect(agent_id: str) -> None:
    """Record the agent id the server assigned on connect."""
    global _agent_id
    _agent_id = agent_id
    _logger.info("echo agent registered as %s", agent_id)


def on_text_frame(text: str) -> str | None:
    """Echo the incoming text frame back verbatim (return-style)."""
    global _echoed
    _echoed += 1
    return text


def on_binary_frame(frame: bytes) -> bytes | None:
    """Echo the incoming binary frame back verbatim (return-style)."""
    global _echoed
    _echoed += 1
    return frame


def on_shutdown() -> None:
    """Log the running echo count as the connection closes."""
    _logger.info("echo agent shutting down after %d frames", _echoed)
