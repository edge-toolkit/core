"""Shared fixtures for black-box MLflow bridge tests."""

import importlib.util
import json
import os
import sys
import threading
import time
import types
import unittest
from contextlib import contextmanager
from pathlib import Path
from unittest.mock import patch

MODULE_PATH = Path(__file__).parents[1] / "pkg" / "et_ws_pyo3_mlflow.py"
STATE_FILE = "pyo3-mlflow-bridge-state.json"


def _load_bridge() -> types.ModuleType:
    """Load an isolated bridge instance without requiring a running PyO3 host."""
    name = "pyo3_mlflow_test_module"
    spec = importlib.util.spec_from_file_location(name, MODULE_PATH)
    if spec is None:
        raise RuntimeError(f"could not create an import spec for {MODULE_PATH}")
    module = importlib.util.module_from_spec(spec)
    if spec.loader is None:
        raise RuntimeError(f"import spec for {MODULE_PATH} has no loader")
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def make_event(**values):
    """Build a valid metric event with optional field overrides for each test."""
    result = {
        "type": "mlflow-event",
        "schema_version": 1,
        "producer": "test-producer",
        "event": "metrics",
        "event_id": "event-1",
        "run_key": "run-1",
        "step": 1,
        "metrics": {"loss": 0.5},
    }
    result.update(values)
    return result


def wait_until(predicate, timeout: float = 2.0) -> None:
    """Wait for an observable worker outcome and fail if it does not arrive within the deadline."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.01)
    raise AssertionError("timed out waiting for bridge behavior")


def timeline_index(timeline, predicate, description: str) -> int:
    """Locate an observable action so tests can assert persistence and acknowledgement ordering."""
    for index, item in enumerate(timeline):
        if predicate(item):
            return index
    raise AssertionError(f"timeline did not contain {description}")


class _MemoryStorage:
    """Model agent-scoped storage while recording reads and writes for black-box assertions."""

    def __init__(self, timeline):
        """Create separate local and remote object stores sharing the test's action timeline."""
        self.values = {}
        self.remote_values = {}
        self.timeline = timeline

    def put(self, name, value):
        """Save a local object and record the exact persisted bytes."""
        payload = bytes(value)
        self.values[name] = payload
        self.timeline.append(("put", name, payload))

    def get(self, agent_id, name):
        """Read from the requested agent's storage without conflating its namespace with another's."""
        self.timeline.append(("get", agent_id, name))
        if agent_id == "mlflow-agent":
            return self.values.get(name)
        return self.remote_values.get((agent_id, name))


class _Sender:
    """Capture outgoing acknowledgements and their order relative to storage operations."""

    def __init__(self, timeline):
        """Create an empty message history linked to the shared action timeline."""
        self.messages = []
        self.timeline = timeline

    def text(self, value):
        """Decode and record an outgoing JSON acknowledgement."""
        message = json.loads(value)
        self.messages.append(message)
        self.timeline.append(("ack", message["status"], message))


class _FakeMlflowClient:
    """Record tracking operations without a network connection or the MLflow dependency."""

    def __init__(self):
        """Create independent histories for run creation, measurements and lifecycle operations."""
        self.created_runs = []
        self.metrics = []
        self.artifacts = []
        self.params = []
        self.tags = []
        self.terminated = []

    @staticmethod
    def get_experiment_by_name(_name):
        """Return the existing experiment used by the default test scenario."""
        return types.SimpleNamespace(experiment_id="experiment-1")

    @staticmethod
    def create_experiment(_name):
        """Return a stable experiment identifier when a test requests experiment creation."""
        return "experiment-1"

    @staticmethod
    def search_runs(**_kwargs):
        """Report no remote matches unless a test overrides the lookup."""
        return []

    def create_run(self, _experiment_id, *, tags):
        """Allocate a distinct run ID and retain the identity tags supplied by the bridge."""
        run_id = f"run-{len(self.created_runs) + 1}"
        self.created_runs.append((run_id, tags))
        return types.SimpleNamespace(info=types.SimpleNamespace(run_id=run_id))

    def log_param(self, run_id, key, value):
        """Record a parameter update for later comparison with the producer's event."""
        self.params.append((run_id, key, value))

    def set_tag(self, run_id, key, value):
        """Record a tag update and its destination run."""
        self.tags.append((run_id, key, value))

    def log_metric(self, run_id, key, value, *, timestamp, step):
        """Record a metric together with its run, timestamp and measurement step."""
        self.metrics.append((run_id, key, value, timestamp, step))

    def log_artifact(self, run_id, local_path, *, artifact_path):
        """Capture uploaded bytes before the bridge removes its temporary artifact file."""
        self.artifacts.append((run_id, Path(local_path).read_bytes(), artifact_path))

    def set_terminated(self, run_id, *, status):
        """Record the terminal status requested for a run."""
        self.terminated.append((run_id, status))


class _FakeMlflowModule(types.ModuleType):
    """Expose the replaceable SDK client factory imported by the bridge worker."""

    MlflowClient: object


class UnexpectedClientError(Exception):
    """Represent an SDK failure outside the advertised operational exception types."""


class BridgeFixtureMixin:
    """Share bridge fixtures and helpers with concrete unittest test classes."""

    def setUp(self):
        """Create an isolated module instance and in-memory host interfaces."""
        if not isinstance(self, unittest.TestCase):
            raise TypeError("BridgeFixtureMixin requires a unittest.TestCase subclass")
        self.timeline = []
        self.storage = _MemoryStorage(self.timeline)
        self.sender = _Sender(self.timeline)
        self.client = _FakeMlflowClient()
        mlflow_module = _FakeMlflowModule("mlflow")
        mlflow_module.MlflowClient = self._create_client
        self.mlflow_patch = patch.dict(sys.modules, {"mlflow": mlflow_module})
        self.mlflow_patch.start()
        self.addCleanup(self.mlflow_patch.stop)
        policy_patch = patch.dict(os.environ, {"PYO3_MLFLOW_ARTIFACT_BUCKETS": ""})
        policy_patch.start()
        self.addCleanup(policy_patch.stop)
        self.module = _load_bridge()
        self.module.init(self.sender, self.storage)
        self.addCleanup(self.module.on_shutdown)

    def _create_client(self, *, tracking_uri):
        """Verify the configured tracking URI reaches client construction and return the test double."""
        if not isinstance(tracking_uri, str):
            raise TypeError("tracking URI must be a string")
        return self.client

    def connect(self):
        """Invoke registration with the storage identity reserved for the bridge in these tests."""
        self.module.on_connect("mlflow-agent")

    def persist_pending(self, event, *, next_attempt_at=0.0):
        """Seed a durable pending event to exercise restoration and deferred processing through public hooks."""
        document = {
            "schema_version": 1,
            "runs": {},
            "processed": [],
            "pending": [{"event": event, "attempts": 0, "next_attempt_at": next_attempt_at}],
        }
        self.storage.put(STATE_FILE, json.dumps(document).encode())

    def wait_for_status(self, status, event_id="event-1"):
        """Wait for an acknowledgement identifying the event and expected outcome."""
        wait_until(
            lambda: any(
                message["event_id"] == event_id and message["status"] == status for message in self.sender.messages
            )
        )

    @contextmanager
    def paused_worker(self):
        """Hold client construction so ingestion failure assertions cannot race background processing."""
        entered = threading.Event()
        release = threading.Event()

        def create_client(*, tracking_uri):
            """Wait for the test to release the worker before exposing the SDK test double."""
            entered.set()
            if not release.wait(timeout=5.0):
                raise RuntimeError("test did not release client construction")
            return self._create_client(tracking_uri=tracking_uri)

        with patch.object(sys.modules["mlflow"], "MlflowClient", side_effect=create_client):
            try:
                self.connect()
                if not entered.wait(timeout=2.0):
                    raise AssertionError("worker did not reach client construction")
                yield
            finally:
                release.set()
