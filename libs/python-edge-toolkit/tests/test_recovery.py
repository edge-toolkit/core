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
        """Create isolated namespaces and controls for checkpoint write failures."""
        self.agent_id: str | None = "worker"
        self.values = {}
        self.namespaces = {"worker": self.values}
        self.reads = []
        self.fail_pointer = False

    def put(self, key, value):
        """Write to the current agent namespace or inject a failed pointer commit."""
        if self.agent_id is None:
            raise RuntimeError("storage agent identity is not assigned")
        if self.fail_pointer and key.endswith("-latest.json"):
            raise OSError("pointer write failed")
        self.namespaces.setdefault(self.agent_id, {})[key] = value

    def get(self, agent_id, key):
        """Record the requested namespace and return its stored object when present."""
        self.reads.append((agent_id, key))
        return self.namespaces.get(agent_id, {}).get(key)


def test_lifecycle_uses_existing_direct_cloudevent_transport():
    """Verify ordered lifecycle events use the existing acknowledged agent-message envelope."""
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
    """Reject incomplete lifecycle bindings before publishing any evidence."""
    with pytest.raises(ValueError, match="required"):
        Lifecycle(Mock(), supervisor, source, job)


def test_lifecycle_rejects_invalid_or_unannounced_evidence():
    """Reject unsupported actions, invalid counters and progress reported before startup."""
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
    """Preserve sequence order across delivery failures and attach checkpoint measurements to lifecycle evidence."""
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
    """Distinguish an absent checkpoint from a successfully verified payload."""
    storage = MemoryStorage()
    checkpoints = Checkpoints(storage, "worker", "model")
    assert checkpoints.load() is None
    checkpoints.save(b"model weights", 3)
    assert checkpoints.load() == (b"model weights", 3)
    assert all(agent == "worker" for agent, _ in storage.reads)
    pointer = json.loads(storage.values["model-latest.json"])
    assert pointer["sha256"] == hashlib.sha256(b"model weights").hexdigest()


def test_checkpoint_success_does_not_depend_on_lifecycle_delivery(caplog):
    """Keep committed checkpoints usable when lifecycle telemetry cannot be delivered."""
    lifecycle = Lifecycle(Mock(), "supervisor", "/modules/example", "job")
    lifecycle.emit("started", 0)
    lifecycle.send = Mock(side_effect=OSError("hub unavailable"))
    checkpoints = Checkpoints(MemoryStorage(), "worker", "model", lifecycle=lifecycle)
    checkpoints.save(b"valid", 1)
    assert checkpoints.load() == (b"valid", 1)
    assert caplog.text.count("checkpoint succeeded but lifecycle delivery failed") == 2


def test_failed_pointer_commit_retains_previous_checkpoint():
    """Preserve the committed payload when writing the next latest pointer fails."""
    storage = MemoryStorage()
    checkpoints = Checkpoints(storage, "worker", "model")
    checkpoints.save(b"committed", 1)
    storage.fail_pointer = True
    with pytest.raises(OSError, match="pointer write failed"):
        checkpoints.save(b"uncommitted", 2)
    assert checkpoints.load() == (b"committed", 1)


@pytest.mark.parametrize("current_agent", [None, "replacement"])
def test_mismatched_or_unregistered_checkpoint_writer_is_rejected_before_storage_access(current_agent):
    """Reject stale writer identities before reading or changing either storage namespace."""
    storage = MemoryStorage()
    checkpoints = Checkpoints(storage, "worker", "model")
    checkpoints.save(b"previous agent", 1)
    storage.agent_id = current_agent
    before = {agent: values.copy() for agent, values in storage.namespaces.items()}
    reads = storage.reads.copy()
    with pytest.raises(ValueError, match="current storage agent identity"):
        checkpoints.save(b"wrong namespace", 2)
    assert storage.reads == reads
    assert storage.namespaces == before
    assert checkpoints.load() == (b"previous agent", 1)


def test_cross_agent_restore_does_not_corrupt_new_agents_checkpoint_on_failed_commit():
    """Restore old-agent state without allowing stale writers to corrupt the new agent's committed checkpoint."""
    storage = MemoryStorage()
    previous = Checkpoints(storage, "worker", "model")
    previous.save(b"old agent", 1)
    storage.agent_id = "replacement"
    assert previous.load() == (b"old agent", 1)
    current = Checkpoints(storage, "replacement", "model")
    current.save(b"new agent committed", 2)
    storage.fail_pointer = True
    with pytest.raises(ValueError, match="current storage agent identity"):
        previous.save(b"would overwrite current slot", 3)
    with pytest.raises(OSError, match="pointer write failed"):
        current.save(b"new agent uncommitted", 3)
    assert current.load() == (b"new agent committed", 2)
    assert previous.load() == (b"old agent", 1)


def test_repeated_saves_and_failed_commits_keep_storage_bounded():
    """Bound payload retention even when saves repeatedly fail before pointer commitment."""
    storage = MemoryStorage()
    checkpoints = Checkpoints(storage, "worker", "model")
    for progress in range(20):
        checkpoints.save(str(progress).encode(), progress)
        assert checkpoints.load() == (str(progress).encode(), progress)
        assert len(storage.values) <= 3
    storage.fail_pointer = True
    for progress in range(20, 30):
        with pytest.raises(OSError, match="pointer write failed"):
            checkpoints.save(str(progress).encode(), progress)
        assert checkpoints.load() == (b"19", 19)
        assert len(storage.values) == 3
    storage.fail_pointer = False
    checkpoints.save(b"resumed", 30)
    assert Checkpoints(storage, "worker", "model").load() == (b"resumed", 30)
    assert len(storage.values) == 3


def test_legacy_pointer_restores_and_next_save_uses_bounded_slots():
    """Read legacy content-addressed checkpoints while bounding storage used by subsequent saves."""
    storage = MemoryStorage()
    payload = b"legacy"
    digest = hashlib.sha256(payload).hexdigest()
    filename = f"model-1-{digest}.bin"
    storage.values[filename] = payload
    storage.values["model-latest.json"] = json.dumps(
        {"schema_version": 1, "filename": filename, "sha256": digest, "progress": 1}
    ).encode()
    checkpoints = Checkpoints(storage, "worker", "model")
    assert checkpoints.load() == (payload, 1)
    for progress in range(2, 10):
        checkpoints.save(b"current", progress)
        assert checkpoints.load() == (b"current", progress)
    assert len(storage.values) == 4


def test_save_does_not_overwrite_malformed_pointer_or_other_payloads():
    """Reject malformed saved pointers without overwriting existing storage objects."""
    storage = MemoryStorage()
    storage.values["model-latest.json"] = b"invalid pointer"
    before = storage.values.copy()
    with pytest.raises(ValueError):
        Checkpoints(storage, "worker", "model").save(b"new", 1)
    assert storage.values == before


@pytest.mark.parametrize(
    ("agent", "key"), [("", "model"), ("worker", ""), ("worker", "../state"), ("w", "x" * 65), ("w", "\u00e9")]
)
def test_checkpoint_names_cannot_escape_the_application_namespace(agent, key):
    """Reject checkpoint keys that could address another application namespace."""
    with pytest.raises(ValueError, match="simple key"):
        Checkpoints(MemoryStorage(), agent, key)


@pytest.mark.parametrize("limit", [0, -1, True, 1.5])
def test_checkpoint_size_limit_is_a_positive_integer(limit):
    """Reject disabled, fractional and Boolean checkpoint size limits."""
    with pytest.raises(ValueError, match="positive integer"):
        Checkpoints(MemoryStorage(), "worker", "model", max_bytes=limit)


def test_checkpoint_save_enforces_payload_and_progress_limits():
    """Reject oversized payloads and counters outside the unsigned checkpoint range."""
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
        {"schema_version": True},
        {"schema_version": 1, "progress": True},
        {"schema_version": 1, "progress": -1},
        {"schema_version": 1, "progress": 2**64},
        {"schema_version": 1, "progress": 0, "sha256": "bad"},
        {"schema_version": 1, "progress": 0, "sha256": "z" * 64},
        {"schema_version": 1, "progress": 0, "sha256": "a" * 64, "filename": "../private"},
        {"schema_version": 1, "progress": 0, "sha256": "a" * 64, "filename": []},
    ],
)
def test_invalid_checkpoint_pointers_fail_before_reading_payload(pointer):
    """Reject unsupported or malformed pointers before attempting a payload read."""
    storage = MemoryStorage()
    storage.values["model-latest.json"] = json.dumps(pointer).encode()
    with pytest.raises(ValueError):
        Checkpoints(storage, "worker", "model").load()
    assert storage.reads == [("worker", "model-latest.json")]


def test_unreadable_and_oversized_checkpoint_pointers_fail_explicitly():
    """Report invalid pointer encoding and excessive pointer size as errors rather than absent checkpoints."""
    storage = MemoryStorage()
    for payload in [b"not json", b"x" * 4097]:
        storage.values["model-latest.json"] = payload
        with pytest.raises(ValueError):
            Checkpoints(storage, "worker", "model").load()


@pytest.mark.parametrize("payload", [None, b"corrupt", b"x" * 20])
def test_missing_corrupt_or_oversized_payloads_cannot_be_restored(payload):
    """Reject payloads that are absent, too large or inconsistent with the committed digest."""
    storage = MemoryStorage()
    checkpoints = Checkpoints(storage, "worker", "model", max_bytes=10)
    checkpoints.save(b"valid", 1)
    pointer = json.loads(storage.values["model-latest.json"])
    storage.values[pointer["filename"]] = payload
    with pytest.raises(ValueError, match="missing, oversized or corrupt"):
        checkpoints.load()
