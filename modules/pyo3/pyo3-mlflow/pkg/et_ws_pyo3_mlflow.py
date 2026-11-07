# noqa: INP001 -- et-ws-pyo3-runner compiles this one file on its own; it is not a package
"""Send tracking events from Edge Toolkit modules to MLflow reliably."""

from __future__ import annotations

import importlib
import json
import logging
import math
import os
import queue
import re
import tempfile
import threading
import time
from contextlib import suppress
from pathlib import Path
from typing import Any

_logger = logging.getLogger(__name__)

EVENT_TYPE = "mlflow-event"
ACK_TYPE = "mlflow-ack"
CURRENT_SCHEMA_VERSION = 1
SUPPORTED_STATUSES = frozenset({"FINISHED", "FAILED", "KILLED"})
SAFE_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_. /:@+-]{0,254}$")
STATE_SCHEMA_VERSION = 1
DEFAULT_STATE_FILE = "pyo3-mlflow-bridge-state.json"

_state: dict[str, Any] = {
    "send": None,
    "storage": None,
    "agent_id": None,
    "queue": None,
    "worker": None,
    "stop": None,
    "lock": threading.RLock(),
    "runs": {},
    "processed": [],
    "pending": {},
}


def init(send, storage) -> None:
    """Save host handles; the worker starts after the hub assigns an agent ID."""
    _state["send"] = send
    _state["storage"] = storage


def on_connect(agent_id: str) -> None:
    """Restore durable state and start one worker after hub registration."""
    _state["agent_id"] = agent_id
    existing = _state.get("worker")
    if existing is not None and existing.is_alive():
        _logger.info("pyo3-mlflow bridge reconnected as %s", agent_id)
        return

    _restore_state()
    work_queue: queue.Queue[dict[str, Any] | None] = queue.Queue(
        maxsize=_positive_int_env("PYO3_MLFLOW_QUEUE_SIZE", 1000)
    )
    stop = threading.Event()
    worker = threading.Thread(target=_worker_main, args=(work_queue, stop), name="pyo3-mlflow", daemon=True)
    _state["queue"] = work_queue
    _state["stop"] = stop
    _state["worker"] = worker
    worker.start()

    # The durable map is authoritative; queue entries are only wake-up hints.
    for item in list(_state["pending"].values()):
        with suppress(queue.Full):
            work_queue.put_nowait(item["event"])
    _logger.info(
        "pyo3-mlflow bridge connected as %s with %d pending event(s)",
        agent_id,
        len(_state["pending"]),
    )


def on_text_frame(text: str) -> None:
    """Validate, durably enqueue and acknowledge one tracking event."""
    if len(text.encode("utf-8")) > _positive_int_env("PYO3_MLFLOW_MAX_EVENT_BYTES", 256 * 1024):
        return
    try:
        value = json.loads(text)
    except (TypeError, ValueError):
        return
    if not isinstance(value, dict) or value.get("type") != EVENT_TYPE:
        return

    event_id = str(value.get("event_id", "unknown"))
    run_key = str(value.get("run_key", "unknown"))
    try:
        event = _validate_event(value)
    except (KeyError, TypeError, ValueError) as exc:
        _ack(event_id, run_key, "rejected", str(exc))
        return

    try:
        _enqueue_event(event)
    except queue.Full:
        _ack(event_id, run_key, "failed", "durable queue full")
    except (AttributeError, OSError, RuntimeError) as exc:
        _logger.exception("failed to persist MLflow event %s", event_id)
        _ack(event_id, run_key, "failed", f"outbox persistence failed: {exc}")


def _enqueue_event(event: dict[str, Any]) -> None:
    """Persist new events before accepting them and recognise previously queued or completed identities."""
    identity = _event_identity(event)
    with _state["lock"]:
        if identity in set(_state["processed"]):
            _ack(event["event_id"], event["run_key"], "duplicate")
            return
        if identity in _state["pending"]:
            _ack(event["event_id"], event["run_key"], "queued")
            return
        if len(_state["pending"]) >= _positive_int_env("PYO3_MLFLOW_QUEUE_SIZE", 1000):
            raise queue.Full
        _state["pending"][identity] = {"event": event, "attempts": 0, "next_attempt_at": 0.0}
        try:
            _persist_state_locked()
        except (AttributeError, OSError, RuntimeError):
            # A rejected write must not become eligible for processing or be saved by a later event.
            _state["pending"].pop(identity, None)
            raise
    # A queue entry only wakes the worker; the persisted pending map owns the event, so a full hint queue cannot
    # lose it.
    with suppress(queue.Full):
        _state["queue"].put_nowait(event)
    _ack(event["event_id"], event["run_key"], "queued")


def on_shutdown() -> None:
    """Stop promptly; pending work has already been persisted for replay."""
    stop = _state.get("stop")
    if stop is not None:
        stop.set()
    work_queue = _state.get("queue")
    if work_queue is not None:
        with suppress(queue.Full):
            work_queue.put_nowait(None)
    worker = _state.get("worker")
    if worker is not None:
        worker.join(timeout=5.0)


def _validate_event(value: dict[str, Any]) -> dict[str, Any]:
    """Return a validated event copy or raise for an unsupported or malformed payload."""
    version = value.get("schema_version")
    if version != CURRENT_SCHEMA_VERSION:
        raise ValueError("unsupported schema_version")
    event = dict(value)
    _required_safe_string(event, "producer")
    kind = _required_safe_string(event, "event")
    if kind not in _EVENT_VALIDATORS:
        raise ValueError(f"unsupported event {kind!r}")
    _required_safe_string(event, "event_id")
    _required_safe_string(event, "run_key")
    for name in ("experiment", "run_name"):
        if name in event:
            _safe_string(event[name], name)
    _EVENT_VALIDATORS[kind](event)
    return event


def _validate_run_start_event(event: dict[str, Any]) -> None:
    """Check optional parameters and tags supplied when a logical run starts."""
    _validate_mapping(event.get("params", {}), "params")
    _validate_mapping(event.get("tags", {}), "tags")


def _validate_params_event(event: dict[str, Any]) -> None:
    """Check that parameter updates contain named scalar values."""
    _validate_mapping(event.get("params", {}), "params")


def _validate_tags_event(event: dict[str, Any]) -> None:
    """Check that tag updates contain named scalar values."""
    _validate_mapping(event.get("tags", {}), "tags")


def _validate_metrics_event(event: dict[str, Any]) -> None:
    """Check metric values and their associated measurement step."""
    _validate_metrics(event.get("metrics"))
    _validate_step(event.get("step", 0))


def _validate_artifacts_event(event: dict[str, Any]) -> None:
    """Check artifact pointers and require operator permission for every source bucket."""
    artifacts = event.get("artifacts")
    if not isinstance(artifacts, list) or not artifacts:
        raise ValueError("artifacts must be a non-empty list")
    for artifact in artifacts:
        if not isinstance(artifact, dict):
            raise TypeError("artifact entries must be objects")
        _required_safe_string(artifact, "bucket")
        _require_allowed_artifact_bucket(artifact["bucket"])
        _required_safe_string(artifact, "filename")
        _required_safe_string(artifact, "artifact_path")


def _validate_run_end_event(event: dict[str, Any]) -> None:
    """Check the terminal status, optional final metrics and measurement step."""
    status = event.get("status", "FINISHED")
    if status not in SUPPORTED_STATUSES:
        raise ValueError("invalid run status")
    if "metrics" in event:
        _validate_metrics(event["metrics"])
    _validate_step(event.get("step", 0))


_EVENT_VALIDATORS = {
    "run-start": _validate_run_start_event,
    "params": _validate_params_event,
    "tags": _validate_tags_event,
    "metrics": _validate_metrics_event,
    "artifacts": _validate_artifacts_event,
    "run-end": _validate_run_end_event,
}


def _worker_main(work_queue: queue.Queue[dict[str, Any] | None], stop: threading.Event) -> None:
    """Initialise the SDK client and process due events until shutdown is requested."""
    try:
        mlflow = importlib.import_module("mlflow")
    except ImportError:
        _logger.exception("mlflow is unavailable; pending events remain in the durable outbox")
        return

    operational_errors = _operational_errors(mlflow)
    try:
        client = mlflow.MlflowClient(tracking_uri=os.environ.get("MLFLOW_TRACKING_URI", "http://127.0.0.1:5000"))
    except operational_errors:
        _logger.exception("MLflow client initialization failed; pending events remain in the durable outbox")
        return
    while not stop.is_set():
        try:
            hint = work_queue.get(timeout=0.5)
            if hint is None:
                break
        except queue.Empty:
            # A notification timeout still allows scheduled retries to be processed.
            pass

        item = _next_pending_item()
        if item is None:
            continue
        identity, record = item
        _process_pending_item(client, operational_errors, identity, record)


def _process_pending_item(client, operational_errors, identity: str, record: dict[str, Any]) -> None:
    """Dispatch one pending event, recording success or isolating its failure."""
    event = record["event"]
    try:
        run_id = _dispatch(client, event)
        _mark_processed(identity)
        _ack(event["event_id"], event["run_key"], "logged", run_id=run_id)
    except operational_errors as exc:
        _handle_operational_failure(identity, record, exc)
    except Exception as exc:  # pylint: disable=broad-exception-caught
        # Deliberately catch ordinary exceptions at the event boundary so a poison event cannot stop other runs.
        # Log and fail the event explicitly; BaseException shutdown signals are not intercepted.
        _logger.exception("unexpected failure processing MLflow event %s; dropping event", event["event_id"])
        _drop_pending(identity)
        _ack(event["event_id"], event["run_key"], "failed", f"unexpected event failure: {exc}")


def _handle_operational_failure(identity: str, record: dict[str, Any], error: Exception) -> None:
    """Schedule a bounded retry or report terminal failure when the attempt limit is reached."""
    event = record["event"]
    attempts = int(record.get("attempts", 0)) + 1
    maximum = _positive_int_env("PYO3_MLFLOW_MAX_RETRIES", 8)
    if attempts >= maximum:
        _logger.exception("MLflow event %s exhausted %d attempts", event["event_id"], attempts)
        _drop_pending(identity)
        _ack(event["event_id"], event["run_key"], "failed", f"retry limit reached: {error}")
        return

    delay = min(
        float(_positive_int_env("PYO3_MLFLOW_RETRY_MAX_SECONDS", 60)),
        float(_positive_int_env("PYO3_MLFLOW_RETRY_BASE_SECONDS", 2)) * (2 ** (attempts - 1)),
    )
    _reschedule(identity, attempts, time.time() + delay)
    _logger.warning(
        "MLflow event %s attempt %d/%d failed; retrying in %.1fs: %s",
        event["event_id"],
        attempts,
        maximum,
        delay,
        error,
    )
    _ack(event["event_id"], event["run_key"], "retrying", str(error))


def _operational_errors(mlflow: Any) -> tuple[type[Exception], ...]:
    """Combine host failure types with the SDK's exception class when one is available."""
    errors: tuple[type[Exception], ...] = (AttributeError, OSError, RuntimeError, TypeError, ValueError)
    mlflow_exception = getattr(getattr(mlflow, "exceptions", None), "MlflowException", None)
    if isinstance(mlflow_exception, type) and issubclass(mlflow_exception, Exception):
        return (*errors, mlflow_exception)
    return errors


def _next_pending_item() -> tuple[str, dict[str, Any]] | None:
    """Select a due event while preserving order within each logical run, or return None when none is due."""
    now = time.time()
    with _state["lock"]:
        # Preserve ordering within each producer/run while allowing an outage
        # in one run to leave unrelated producers free to make progress.
        eligible = []
        observed_runs: set[str] = set()
        for identity, record in _state["pending"].items():
            run_identity = _run_identity(record["event"])
            if run_identity in observed_runs:
                continue
            observed_runs.add(run_identity)
            if float(record.get("next_attempt_at", 0.0)) <= now:
                eligible.append((identity, record))
        if not eligible:
            return None
        return min(eligible, key=lambda item: float(item[1].get("next_attempt_at", 0.0)))


def _dispatch(client, event: dict[str, Any]) -> str:
    """Apply a validated tracking operation and return its MLflow run ID."""
    run_id = _ensure_run(client, event)
    kind = event["event"]
    if kind in {"run-start", "params"}:
        for key, value in event.get("params", {}).items():
            client.log_param(run_id, str(key), str(value))
    if kind in {"run-start", "tags"}:
        for key, value in event.get("tags", {}).items():
            client.set_tag(run_id, str(key), str(value))
    if kind == "metrics":
        _log_metrics(client, run_id, event["metrics"], int(event.get("step", 0)))
    elif kind == "artifacts":
        for artifact in event["artifacts"]:
            _log_artifact(client, run_id, artifact)
    elif kind == "run-end":
        if "metrics" in event:
            _log_metrics(client, run_id, event["metrics"], int(event.get("step", 0)))
        client.set_terminated(run_id, status=event.get("status", "FINISHED"))
    return run_id


def _ensure_run(client, event: dict[str, Any]) -> str:
    """Reuse a local or remotely discovered run, creating one only when no match exists."""
    identity = _run_identity(event)
    with _state["lock"]:
        existing = _state["runs"].get(identity)
    if existing:
        return existing

    experiment_name = event.get("experiment") or os.environ.get("MLFLOW_DEFAULT_EXPERIMENT", "edge-toolkit")
    experiment = client.get_experiment_by_name(experiment_name)
    experiment_id = experiment.experiment_id if experiment is not None else client.create_experiment(experiment_name)
    escaped = identity.replace("\\", "\\\\").replace("'", "\\'")
    runs = client.search_runs(
        experiment_ids=[experiment_id],
        filter_string=f"tags.`edge_toolkit.run_identity` = '{escaped}'",
        max_results=1,
        order_by=["attribute.start_time DESC"],
    )
    if runs:
        run_id = runs[0].info.run_id
    else:
        tags = {
            "edge_toolkit.producer": event["producer"],
            "edge_toolkit.run_key": event["run_key"],
            "edge_toolkit.run_identity": identity,
            "mlflow.runName": event.get("run_name", event["run_key"]),
        }
        run_id = client.create_run(experiment_id, tags=tags).info.run_id
    with _state["lock"]:
        _state["runs"][identity] = run_id
        _persist_state_locked()
    return run_id


def _log_metrics(client, run_id: str, metrics: dict[str, Any], step: int) -> None:
    """Record metric values at one measurement step with a shared timestamp."""
    timestamp = int(time.time() * 1000)
    for key, value in metrics.items():
        client.log_metric(run_id, str(key), float(value), timestamp=timestamp, step=step)


def _log_artifact(client, run_id: str, artifact: dict[str, Any]) -> None:
    """Upload an authorised storage object through a temporary file, enforcing the payload size limit."""
    # Recheck persisted events against the current policy before performing any storage read.
    _require_allowed_artifact_bucket(artifact["bucket"])
    data = _state["storage"].get(artifact["bucket"], artifact["filename"])
    if data is None:
        raise RuntimeError(f"artifact {artifact['bucket']}/{artifact['filename']} was not found")
    payload = bytes(data)
    maximum = _positive_int_env("PYO3_MLFLOW_MAX_ARTIFACT_BYTES", 50 * 1024 * 1024)
    if len(payload) > maximum:
        raise ValueError(f"artifact exceeds {maximum} bytes")
    destination = Path(artifact["artifact_path"])
    artifact_dir = str(destination.parent) if destination.parent != Path(".") else None
    with tempfile.TemporaryDirectory(prefix="et-mlflow-") as temporary:
        local_path = Path(temporary) / destination.name
        local_path.write_bytes(payload)
        client.log_artifact(run_id, str(local_path), artifact_path=artifact_dir)


def _require_allowed_artifact_bucket(bucket: str) -> None:
    """Reject unlisted source buckets and all reads from the bridge's own storage area."""
    allowed = {name.strip() for name in os.environ.get("PYO3_MLFLOW_ARTIFACT_BUCKETS", "").split(",") if name.strip()}
    if bucket == _state.get("agent_id") or bucket not in allowed:
        raise ValueError("artifact bucket is not allowed")


def _event_identity(event: dict[str, Any]) -> str:
    """Build the deduplication key from the producer, logical run and event identifiers."""
    return f"{event['producer']}::{event['run_key']}::{event['event_id']}"


def _run_identity(event: dict[str, Any]) -> str:
    """Build the producer-scoped identifier used to find and reuse a logical run."""
    return f"{event['producer']}::{event['run_key']}"


def _mark_processed(identity: str) -> None:
    """Persist event completion and retain only the configured amount of deduplication history."""
    with _state["lock"]:
        previous_pending = dict(_state["pending"])
        previous_processed = _state["processed"]
        _state["pending"].pop(identity, None)
        processed = [item for item in _state["processed"] if item != identity]
        processed.append(identity)
        limit = _positive_int_env("PYO3_MLFLOW_DEDUP_HISTORY", 10_000)
        _state["processed"] = processed[-limit:]
        try:
            _persist_state_locked()
        except (AttributeError, OSError, RuntimeError, TypeError, ValueError):
            # Preserve pending order and completion history until the transition is durably accepted.
            _state["pending"] = previous_pending
            _state["processed"] = previous_processed
            raise


def _drop_pending(identity: str) -> None:
    """Remove a terminally failed event from the persisted pending set."""
    with _state["lock"]:
        _state["pending"].pop(identity, None)
        _persist_state_locked()


def _reschedule(identity: str, attempts: int, next_attempt_at: float) -> None:
    """Persist the attempt count and next eligible time for an event that is still pending."""
    with _state["lock"]:
        record = _state["pending"].get(identity)
        if record is None:
            return
        record["attempts"] = attempts
        record["next_attempt_at"] = next_attempt_at
        _persist_state_locked()


def _restore_state() -> None:
    """Load saved runs and event history from agent storage, reporting unreadable or incompatible state."""
    storage = _state.get("storage")
    agent_id = _state.get("agent_id")
    if storage is None or not agent_id:
        return
    try:
        payload = storage.get(agent_id, _state_filename())
        if payload is None:
            return
        document = json.loads(bytes(payload).decode("utf-8"))
        if document.get("schema_version") != STATE_SCHEMA_VERSION:
            raise ValueError("unsupported bridge state schema")
        with _state["lock"]:
            _state["runs"] = dict(document.get("runs", {}))
            _state["processed"] = list(document.get("processed", []))
            _state["pending"] = _restore_pending(document.get("pending", []))
    except (AttributeError, OSError, RuntimeError, TypeError, ValueError):
        _logger.exception("failed to restore MLflow bridge state; starting with an empty outbox")
        with _state["lock"]:
            _state["runs"] = {}
            _state["processed"] = []
            _state["pending"] = {}


def _restore_pending(records: list[Any]) -> dict[str, Any]:
    """Recover queued records with valid event identities without discarding unrelated saved work."""
    pending = {}
    for record in records:
        try:
            if not isinstance(record, dict) or not isinstance(record.get("event"), dict):
                raise TypeError("pending record must contain an event object")
            for key in ("producer", "run_key", "event_id"):
                _required_safe_string(record["event"], key)
        except (KeyError, TypeError, ValueError):
            _logger.warning("skipping malformed pending event record during state restoration")
            continue
        pending[_event_identity(record["event"])] = record
    return pending


def _persist_state_locked() -> None:
    """Write the current state to agent storage while the caller holds the state lock."""
    storage = _state.get("storage")
    if storage is None or not _state.get("agent_id"):
        return
    document = {
        "schema_version": STATE_SCHEMA_VERSION,
        "runs": _state["runs"],
        "processed": _state["processed"],
        "pending": list(_state["pending"].values()),
    }
    storage.put(
        _state_filename(),
        json.dumps(document, separators=(",", ":"), sort_keys=True).encode("utf-8"),
    )


def _state_filename() -> str:
    """Return the configured storage key for the bridge's durable state."""
    return os.environ.get("PYO3_MLFLOW_STATE_FILE", DEFAULT_STATE_FILE)


def _ack(
    event_id: str,
    run_key: str,
    status: str,
    message: str | None = None,
    *,
    run_id: str | None = None,
) -> None:
    """Send an event acknowledgement with optional diagnostic text and an MLflow run ID."""
    send = _state.get("send")
    if send is None:
        return
    value = {"type": ACK_TYPE, "event_id": event_id, "run_key": run_key, "status": status}
    if message:
        value["message"] = message[:500]
    if run_id:
        value["mlflow_run_id"] = run_id
    send.text(json.dumps(value, separators=(",", ":"), sort_keys=True))


def _validate_metrics(value: Any) -> None:
    """Require a non-empty mapping of valid names to finite numeric measurements, excluding booleans."""
    if not isinstance(value, dict) or not value:
        raise ValueError("metrics must be a non-empty object")
    for key, metric in value.items():
        _safe_string(str(key), "metric name")
        if isinstance(metric, bool) or not isinstance(metric, (int, float)) or not math.isfinite(float(metric)):
            raise ValueError(f"metric {key!r} must be finite and numeric")


def _validate_mapping(value: Any, name: str) -> None:
    """Require a named mapping whose keys are valid names and whose values are finite scalars."""
    if not isinstance(value, dict):
        raise TypeError(f"{name} must be an object")
    for key, item in value.items():
        _safe_string(str(key), f"{name} key")
        if not isinstance(item, (str, int, float, bool)) or (isinstance(item, float) and not math.isfinite(item)):
            raise ValueError(f"{name} values must be finite scalars")


def _validate_step(value: Any) -> None:
    """Require a non-negative integer measurement step, excluding booleans."""
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise ValueError("step must be a non-negative integer")


def _required_safe_string(value: dict[str, Any], key: str) -> str:
    """Read a required event field and validate it as a permitted name."""
    if key not in value:
        raise KeyError(f"missing {key}")
    return _safe_string(value[key], key)


def _safe_string(value: Any, name: str) -> str:
    """Validate a bounded event name against the permitted characters and reject parent-directory markers."""
    if not isinstance(value, str) or not SAFE_NAME.fullmatch(value) or ".." in value:
        raise ValueError(f"invalid {name}")
    return value


def _positive_int_env(name: str, default: int) -> int:
    """Read a positive integer setting, using the default for missing, malformed or non-positive values."""
    try:
        value = int(os.environ.get(name, str(default)))
    except ValueError:
        return default
    return value if value > 0 else default
