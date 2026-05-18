"""Example Python module for `et-ws-pyo3-runner`.

This file demonstrates the contract the runner expects. Every function is
optional — if your module doesn't define it, the runner skips that hook.

Lifecycle, in order:

  state = init()                   # once, before the websocket connects
  set_agent_id(state, agent_id)    # once, after et-connect-ack
  handle_text(state, text)         # per inbound text frame the et hub didn't
                                   # recognise as a typed et-* WsMessage
  handle_binary(state, frame)      # per inbound binary frame
  shutdown(state)                  # once, after the websocket closes

`handle_text` / `handle_binary` may return a `str` or `bytes` respectively to
send a reply, or `None` for silence. The reply is sent as the same kind of
frame the runner received; under the aligned et-ws-server protocol an
unrecognised reply is default-broadcast back to every other connected agent.

To use this module:

  WS_SERVER_URL=ws://127.0.0.1:8080/ws \\
  PYO3_AGENT_MODULE=echo \\
  PYO3_AGENT_PYTHONPATH=services/ws-pyo3-runner/python \\
  cargo run -p et-ws-pyo3-runner
"""

from __future__ import annotations

import logging

_logger = logging.getLogger(__name__)


def init() -> dict:
    """Build the agent state. Return any object — the runner treats it as
    opaque and passes it back on every subsequent call."""
    _logger.info("echo agent initialised")
    return {"agent_id": None, "echoed": 0}


def set_agent_id(state: dict, agent_id: str) -> None:
    """Receive the agent_id assigned by et-ws-server."""
    state["agent_id"] = agent_id
    _logger.info("echo agent registered as %s", agent_id)


def handle_text(state: dict, text: str) -> str | None:
    """Echo the incoming text frame back verbatim."""
    state["echoed"] += 1
    return text


def handle_binary(state: dict, frame: bytes) -> bytes | None:
    """Echo the incoming binary frame back verbatim."""
    state["echoed"] += 1
    return frame


def shutdown(state: dict) -> None:
    _logger.info("echo agent shutting down after %d frames", state["echoed"])
