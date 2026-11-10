"""Persistent workers, for a native Python ws-module that reads a device for as long as it runs.

A worker is a daemon thread the module's `init` starts. It waits for `on_connect` to give the agent its storage
bucket, then works until the runner shuts down; a module takes the three hooks `persistent_workers` returns as its own:

    init, on_connect, on_shutdown = persistent_workers({"lidar": read_lidar})
"""

from __future__ import annotations

import threading
from collections.abc import Callable
from typing import Any

Work = Callable[[Any, threading.Event], None]
"""What a worker runs: given the runner's storage handle, and an event set once the runner shuts down."""

_CONNECT_POLL_S = 0.1


def persistent_workers(
    works: dict[str, Work], join_timeout: float = 1.0
) -> tuple[Callable[[Any, Any], None], Callable[[str], None], Callable[[], None]]:
    """Return the `init`, `on_connect` and `on_shutdown` hooks that run each of `works` on a thread of its name.

    `on_shutdown` waits up to `join_timeout` seconds for each worker, which checks its event between reads.
    """
    connected = threading.Event()
    stopping = threading.Event()
    threads: list[threading.Thread] = []

    def run(work: Work, storage: Any) -> None:
        while not connected.wait(_CONNECT_POLL_S):
            if stopping.is_set():
                return
        work(storage, stopping)

    def init(_send: Any, storage: Any) -> None:
        for name, work in works.items():
            thread = threading.Thread(target=run, args=(work, storage), name=name, daemon=True)
            threads.append(thread)
            thread.start()

    def on_connect(_agent_id: str) -> None:
        connected.set()

    def on_shutdown() -> None:
        stopping.set()
        for thread in threads:
            thread.join(timeout=join_timeout)

    return init, on_connect, on_shutdown
