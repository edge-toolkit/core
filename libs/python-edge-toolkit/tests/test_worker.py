"""The persistent workers a native Python ws-module's hooks start, connect and stop."""

import threading

from edge_toolkit.worker import persistent_workers

WAIT_S = 5.0


def test_a_worker_waits_for_its_bucket_then_works_until_shutdown() -> None:
    storage = object()
    began = threading.Event()
    seen: list[object] = []

    def work(given: object, stopping: threading.Event) -> None:
        seen.append(given)
        began.set()
        stopping.wait(WAIT_S)

    init, on_connect, on_shutdown = persistent_workers({"work": work})
    init(None, storage)
    assert not began.wait(0.3)
    on_connect("agent-1")
    assert began.wait(WAIT_S)
    on_shutdown()
    assert seen == [storage]


def test_a_worker_shut_down_before_its_bucket_never_works() -> None:
    worked = threading.Event()

    def work(_storage: object, _stopping: threading.Event) -> None:
        worked.set()

    init, _on_connect, on_shutdown = persistent_workers({"work": work}, join_timeout=WAIT_S)
    init(None, object())
    on_shutdown()
    assert not worked.is_set()
