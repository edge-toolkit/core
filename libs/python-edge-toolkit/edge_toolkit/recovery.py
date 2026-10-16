"""Application lifecycle evidence and integrity-checked checkpoints for Core recovery supervision."""

from __future__ import annotations

import hashlib
import json
import logging
import threading
import time
import uuid
from collections.abc import Callable
from typing import Protocol

from et_ws.messages import WsSendAgentMessage

from edge_toolkit.events import client_event


class Storage(Protocol):
    """Agent-scoped storage operations provided by the native Python runner."""

    def put(self, key: str, data: bytes, /) -> None:
        """Persist a whole object in this agent's storage area."""

    def get(self, agent_id: str, key: str, /) -> bytes | None:
        """Retrieve a whole object, or return None when it does not exist."""


class Lifecycle:
    """Publish ordered recovery evidence as direct CloudEvents over the existing hub message transport."""

    def __init__(self, send: Callable[[str], object], supervisor: str, source: str, job_id: str):
        """Bind one application process to a supervisor, module source and job identity."""
        if not supervisor or not source.startswith("/modules/") or not job_id:
            raise ValueError("supervisor, module source and job identity are required")
        self.send = send
        self.supervisor = supervisor
        self.source = source
        self.job_id = job_id
        self.instance = str(uuid.uuid4())
        self.sequence = 0
        self.lock = threading.Lock()

    def emit(self, action: str, progress: int, *, bytes_count: int = 0, duration_millis: int = 0) -> None:
        """Report lifecycle or checkpoint evidence; progress counts committed useful work, not attempts."""
        if action not in {
            "started",
            "progress",
            "failed",
            "completed",
            "checkpoint-saved",
            "checkpoint-restored",
            "resume-completed",
        }:
            raise ValueError("unsupported recovery lifecycle action")
        if isinstance(progress, bool) or not isinstance(progress, int) or not 0 <= progress <= 2**64 - 1:
            raise ValueError("progress must be an unsigned 64-bit integer")
        for value in (bytes_count, duration_millis):
            if isinstance(value, bool) or not isinstance(value, int) or not 0 <= value <= 2**64 - 1:
                raise ValueError("checkpoint measurements must be unsigned 64-bit integers")
        with self.lock:
            if self.sequence == 0 and action != "started":
                raise ValueError("the process must announce started before reporting other lifecycle events")
            sequence = self.sequence + 1
            event = client_event(
                self.source,
                "recovery",
                action,
                {
                    "schema_version": 1,
                    "job_id": self.job_id,
                    "instance": self.instance,
                    "sequence": sequence,
                    "progress": progress,
                    "bytes_count": bytes_count,
                    "duration_millis": duration_millis,
                },
            ).event
            message = WsSendAgentMessage(
                type="et-send-agent-message",
                to_agent_id=self.supervisor,
                message=event.model_dump(mode="json"),
            )
            self.send(message.model_dump_json())
            self.sequence = sequence


class Checkpoints:
    """Commit immutable payloads through a latest pointer; application code owns state format and resume safety."""

    def __init__(
        self,
        storage: Storage,
        agent_id: str,
        key: str,
        max_bytes: int = 64 * 1024 * 1024,
        lifecycle: Lifecycle | None = None,
    ):
        """Bind checkpoints to the producing agent's storage and a bounded application-specific key."""
        if not agent_id or not key or len(key) > 64 or not all(c.isascii() and (c.isalnum() or c in "-_") for c in key):
            raise ValueError("checkpoint identity and a simple key of at most 64 characters are required")
        if isinstance(max_bytes, bool) or not isinstance(max_bytes, int) or max_bytes <= 0:
            raise ValueError("max_bytes must be a positive integer")
        self.storage = storage
        self.agent_id = agent_id
        self.key = key
        self.max_bytes = max_bytes
        self.lifecycle = lifecycle

    def save(self, payload: bytes, progress: int) -> None:
        """Write bytes before the pointer so a failed write cannot replace the previous committed checkpoint."""
        started = time.monotonic_ns()
        if not isinstance(payload, bytes) or len(payload) > self.max_bytes:
            raise ValueError("checkpoint payload must be bytes within the configured size limit")
        if isinstance(progress, bool) or not isinstance(progress, int) or not 0 <= progress <= 2**64 - 1:
            raise ValueError("checkpoint progress must be an unsigned 64-bit integer")
        digest = hashlib.sha256(payload).hexdigest()
        filename = f"{self.key}-{progress}-{digest}.bin"
        self.storage.put(filename, payload)
        pointer = {"schema_version": 1, "filename": filename, "sha256": digest, "progress": progress}
        self.storage.put(f"{self.key}-latest.json", json.dumps(pointer).encode())
        self._observe("checkpoint-saved", progress, len(payload), started)

    def load(self) -> tuple[bytes, int] | None:
        """Verify the latest committed payload; return None only when no checkpoint pointer exists."""
        started = time.monotonic_ns()
        raw = self.storage.get(self.agent_id, f"{self.key}-latest.json")
        if raw is None:
            return None
        if len(raw) > 4096:
            raise ValueError("checkpoint pointer exceeds its size limit")
        pointer = json.loads(raw)
        if (
            not isinstance(pointer, dict)
            or type(pointer.get("schema_version")) is not int
            or pointer["schema_version"] != 1
        ):
            raise ValueError("unsupported checkpoint pointer")
        progress, digest = pointer.get("progress"), pointer.get("sha256")
        if isinstance(progress, bool) or not isinstance(progress, int) or not 0 <= progress <= 2**64 - 1:
            raise ValueError("invalid checkpoint progress")
        if not isinstance(digest, str) or len(digest) != 64 or any(c not in "0123456789abcdef" for c in digest):
            raise ValueError("invalid checkpoint digest")
        filename = f"{self.key}-{progress}-{digest}.bin"
        if pointer.get("filename") != filename:
            raise ValueError("checkpoint pointer does not reference its own payload namespace")
        payload = self.storage.get(self.agent_id, filename)
        if payload is None or len(payload) > self.max_bytes or hashlib.sha256(payload).hexdigest() != digest:
            raise ValueError("checkpoint payload is missing, oversized or corrupt")
        self._observe("checkpoint-restored", progress, len(payload), started)
        return bytes(payload), progress

    def _observe(self, action: str, progress: int, size: int, started: int) -> None:
        """Keep telemetry transport failures from invalidating a successful checkpoint operation."""
        if self.lifecycle is not None:
            try:
                self.lifecycle.emit(
                    action, progress, bytes_count=size, duration_millis=(time.monotonic_ns() - started) // 1_000_000
                )
            except (OSError, RuntimeError, ValueError):
                logging.getLogger(__name__).exception("checkpoint succeeded but lifecycle delivery failed")
