"""Modules publish lifecycle evidence and preserve committed checkpoints through storage failures."""

import hashlib
import json
from typing import Any
from unittest.mock import Mock

import pytest
from edge_toolkit.recovery import Checkpoints, Lifecycle


class MemoryStorage:
    """Model the runner storage contract and allow precise failure injection."""

    def __init__(self):
        self.values = {}
        self.reads = []
        self.fail_pointer = False

    def put(self, key, value):
        if self.fail_pointer and key.endswith("-latest.json"):
            raise OSError("pointer write failed")
        self.values[key] = value

    def get(self, agent_id, key):
        self.reads.append((agent_id, key))
        return self.values.get(key)


def test_lifecycle_uses_existing_direct_cloudevent_transport():
    sent = []
    lifecycle = Lifecycle(sent.append, "supervisor", "/modules/example", "job")
    for action, progress in [("started", 0), ("progress", 1), ("failed", 1), ("completed", 2)]:
        lifecycle.emit(action, progress)
    messages = [json.loads(payload) for payload in sent]
    assert all(message["to_agent_id"] == "supervisor" for message in messages)
    assert all(message["type"] == "et-send-agent-message" for message in messages)
    events = [message["message"] for message in messages]
    assert [event["data"]["sequence"] for event in events] == [1, 2, 3, 4]
    assert [event["type"] for event in events] == [
        "et.recovery.started",
        "et.recovery.progress",
        "et.recovery.failed",
        "et.recovery.completed",
    ]
    assert all(event["source"] == "/modules/example" and event["specversion"] == "1.0" for event in events)
    assert len({event["data"]["instance"] for event in events}) == 1
    assert Lifecycle(sent.append, "supervisor", "/modules/example", "job").instance != lifecycle.instance


@pytest.mark.parametrize(
    ("supervisor", "source", "job"), [("", "/modules/x", "job"), ("s", "x", "job"), ("s", "/modules/x", "")]
)
def test_lifecycle_requires_explicit_identity(supervisor, source, job):
    with pytest.raises(ValueError, match="required"):
        Lifecycle(Mock(), supervisor, source, job)


def test_lifecycle_rejects_invalid_or_unannounced_evidence():
    send = Mock()
    lifecycle = Lifecycle(send, "supervisor", "/modules/example", "job")
    with pytest.raises(ValueError, match="announce started"):
        lifecycle.emit("progress", 1)
    with pytest.raises(ValueError, match="unsupported"):
        lifecycle.emit("restart-arbitrary-deployment", 1)
    invalid_progress: list[Any] = [True, -1, 1.5, 2**64]
    for progress in invalid_progress:
        with pytest.raises(ValueError, match="unsigned"):
            lifecycle.emit("started", progress)
    send.assert_not_called()


def test_checkpoint_measurements_share_lifecycle_and_failed_send_does_not_advance_sequence():
    sent = []
    send = Mock(side_effect=OSError("transport unavailable"))
    lifecycle = Lifecycle(send, "supervisor", "/modules/example", "job")
    with pytest.raises(OSError):
        lifecycle.emit("started", 0)
    assert lifecycle.sequence == 0
    lifecycle.send = sent.append
    lifecycle.emit("started", 0)
    invalid_measurements: list[Any] = [True, -1, "bad", 2**64]
    for value in invalid_measurements:
        with pytest.raises(ValueError, match="measurements"):
            lifecycle.emit("checkpoint-saved", 0, bytes_count=value)
    checkpoints = Checkpoints(MemoryStorage(), "worker", "model", lifecycle=lifecycle)
    checkpoints.save(b"weights", 1)
    assert checkpoints.load() == (b"weights", 1)
    evidence = [json.loads(payload)["message"] for payload in sent]
    assert [item["type"] for item in evidence] == [
        "et.recovery.started",
        "et.recovery.checkpoint-saved",
        "et.recovery.checkpoint-restored",
    ]
    assert all(item["data"]["bytes_count"] == 7 for item in evidence[1:])
    assert all(item["data"]["duration_millis"] >= 0 for item in evidence[1:])


def test_checkpoint_roundtrip_and_no_pointer_are_distinct():
    storage = MemoryStorage()
    checkpoints = Checkpoints(storage, "worker", "model")
    assert checkpoints.load() is None
    checkpoints.save(b"model weights", 3)
    assert checkpoints.load() == (b"model weights", 3)
    assert all(agent == "worker" for agent, _ in storage.reads)
    pointer = json.loads(storage.values["model-latest.json"])
    assert pointer["sha256"] == hashlib.sha256(b"model weights").hexdigest()


def test_checkpoint_success_does_not_depend_on_lifecycle_delivery(caplog):
    lifecycle = Lifecycle(Mock(), "supervisor", "/modules/example", "job")
    lifecycle.emit("started", 0)
    lifecycle.send = Mock(side_effect=OSError("hub unavailable"))
    checkpoints = Checkpoints(MemoryStorage(), "worker", "model", lifecycle=lifecycle)
    checkpoints.save(b"valid", 1)
    assert checkpoints.load() == (b"valid", 1)
    assert caplog.text.count("checkpoint succeeded but lifecycle delivery failed") == 2


def test_failed_pointer_commit_retains_previous_checkpoint():
    storage = MemoryStorage()
    checkpoints = Checkpoints(storage, "worker", "model")
    checkpoints.save(b"committed", 1)
    storage.fail_pointer = True
    with pytest.raises(OSError, match="pointer write failed"):
        checkpoints.save(b"uncommitted", 2)
    assert checkpoints.load() == (b"committed", 1)


@pytest.mark.parametrize(
    ("agent", "key"), [("", "model"), ("worker", ""), ("worker", "../state"), ("w", "x" * 65), ("w", "\u00e9")]
)
def test_checkpoint_names_cannot_escape_the_application_namespace(agent, key):
    with pytest.raises(ValueError, match="simple key"):
        Checkpoints(MemoryStorage(), agent, key)


@pytest.mark.parametrize("limit", [0, -1, True, 1.5])
def test_checkpoint_size_limit_is_a_positive_integer(limit):
    with pytest.raises(ValueError, match="positive integer"):
        Checkpoints(MemoryStorage(), "worker", "model", max_bytes=limit)


def test_checkpoint_save_enforces_payload_and_progress_limits():
    checkpoints = Checkpoints(MemoryStorage(), "worker", "model", max_bytes=3)
    invalid_payloads: list[Any] = ["not bytes", b"too large"]
    for payload in invalid_payloads:
        with pytest.raises(ValueError, match="payload"):
            checkpoints.save(payload, 1)
    invalid_progress: list[Any] = [True, -1, "one", 2**64]
    for progress in invalid_progress:
        with pytest.raises(ValueError, match="progress"):
            checkpoints.save(b"ok", progress)


@pytest.mark.parametrize(
    "pointer",
    [
        [],
        {"schema_version": 2},
        {"schema_version": 1, "progress": True},
        {"schema_version": 1, "progress": -1},
        {"schema_version": 1, "progress": 2**64},
        {"schema_version": 1, "progress": 0, "sha256": "bad"},
        {"schema_version": 1, "progress": 0, "sha256": "z" * 64},
        {"schema_version": 1, "progress": 0, "sha256": "a" * 64, "filename": "../private"},
    ],
)
def test_invalid_checkpoint_pointers_fail_before_reading_payload(pointer):
    storage = MemoryStorage()
    storage.values["model-latest.json"] = json.dumps(pointer).encode()
    with pytest.raises(ValueError):
        Checkpoints(storage, "worker", "model").load()
    assert storage.reads == [("worker", "model-latest.json")]


def test_unreadable_and_oversized_checkpoint_pointers_fail_explicitly():
    storage = MemoryStorage()
    for payload in [b"not json", b"x" * 4097]:
        storage.values["model-latest.json"] = payload
        with pytest.raises(ValueError):
            Checkpoints(storage, "worker", "model").load()


@pytest.mark.parametrize("payload", [None, b"corrupt", b"x" * 20])
def test_missing_corrupt_or_oversized_payloads_cannot_be_restored(payload):
    storage = MemoryStorage()
    checkpoints = Checkpoints(storage, "worker", "model", max_bytes=10)
    checkpoints.save(b"valid", 1)
    pointer = json.loads(storage.values["model-latest.json"])
    storage.values[pointer["filename"]] = payload
    with pytest.raises(ValueError, match="missing, oversized or corrupt"):
        checkpoints.load()
