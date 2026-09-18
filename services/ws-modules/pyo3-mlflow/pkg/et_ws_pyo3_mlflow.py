"""Generic Edge Toolkit event-to-MLflow tracking bridge.

The module listens for ``mlflow-event`` JSON frames, performs MLflow calls on a
background thread, and optionally acknowledges each event. Producers therefore
need no MLflow dependency and never block their workload on observability.
Artifacts are accepted only as Edge Toolkit storage pointers; arbitrary local
paths are deliberately unsupported.
"""

from __future__ import annotations

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
SCHEMA_VERSION = 1
SUPPORTED_EVENTS = frozenset({"run-start", "metrics", "artifacts", "run-end"})
SUPPORTED_STATUSES = frozenset({"FINISHED", "FAILED", "KILLED"})
SAFE_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_. /:@+-]{0,254}$")

_state: dict[str, Any] = {
    "send": None,
    "storage": None,
    "agent_id": None,
    "queue": None,
    "worker": None,
    "stop": None,
    "runs": {},
    "processed": set(),
}


def init(send, storage) -> None:
    """Save host handles; the worker starts after the hub assigns an agent id."""
    _state["send"] = send
    _state["storage"] = storage


def on_connect(agent_id: str) -> None:
    """Start the single MLflow worker after registration."""
    _state["agent_id"] = agent_id
    work_queue: queue.Queue[dict[str, Any] | None] = queue.Queue(
        maxsize=_positive_int_env("PYO3_MLFLOW_QUEUE_SIZE", 1000)
    )
    stop = threading.Event()
    worker = threading.Thread(target=_worker_main, args=(work_queue, stop), name="pyo3-mlflow", daemon=True)
    _state["queue"] = work_queue
    _state["stop"] = stop
    _state["worker"] = worker
    worker.start()
    _logger.info("pyo3-mlflow bridge connected as %s", agent_id)


def on_text_frame(text: str) -> None:
    """Validate and enqueue one tracking event without blocking the WS dispatcher."""
    if len(text.encode("utf-8")) > _positive_int_env("PYO3_MLFLOW_MAX_EVENT_BYTES", 256 * 1024):
        return
    try:
        value = json.loads(text)
    except (TypeError, ValueError):
        return
    if not isinstance(value, dict) or value.get("type") != EVENT_TYPE:
        return
    try:
        event = _validate_event(value)
        _state["queue"].put_nowait(event)
    except queue.Full:
        _ack(str(value.get("event_id", "unknown")), str(value.get("run_key", "unknown")), "failed", "queue full")
    except (KeyError, TypeError, ValueError) as exc:
        _ack(str(value.get("event_id", "unknown")), str(value.get("run_key", "unknown")), "rejected", str(exc))


def on_shutdown() -> None:
    """Ask the worker to finish promptly without hanging runner shutdown."""
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
    if value.get("schema_version") != SCHEMA_VERSION:
        raise ValueError("unsupported schema_version")
    event = _required_safe_string(value, "event")
    if event not in SUPPORTED_EVENTS:
        raise ValueError(f"unsupported event {event!r}")
    _required_safe_string(value, "event_id")
    _required_safe_string(value, "run_key")
    if "experiment" in value:
        _safe_string(value["experiment"], "experiment")
    if "run_name" in value:
        _safe_string(value["run_name"], "run_name")
    if event == "metrics":
        _validate_metrics(value.get("metrics"))
    if event == "run-start":
        _validate_mapping(value.get("params", {}), "params")
        _validate_mapping(value.get("tags", {}), "tags")
    if event == "artifacts":
        artifacts = value.get("artifacts")
        if not isinstance(artifacts, list) or not artifacts:
            raise ValueError("artifacts must be a non-empty list")
        for artifact in artifacts:
            if not isinstance(artifact, dict):
                raise TypeError("artifact entries must be objects")
            _required_safe_string(artifact, "bucket")
            _required_safe_string(artifact, "filename")
            _required_safe_string(artifact, "artifact_path")
    if event == "run-end":
        status = value.get("status", "FINISHED")
        if status not in SUPPORTED_STATUSES:
            raise ValueError("invalid run status")
        if "metrics" in value:
            _validate_metrics(value["metrics"])
    return value


def _worker_main(work_queue: queue.Queue[dict[str, Any] | None], stop: threading.Event) -> None:
    try:
        from mlflow import MlflowClient  # noqa: PLC0415 -- optional runtime dependency, isolated to the worker
    except Exception:
        _logger.exception("mlflow is unavailable; install it in the pyo3 runner environment")
        return

    tracking_uri = os.environ.get("MLFLOW_TRACKING_URI", "http://127.0.0.1:5000")
    client = MlflowClient(tracking_uri=tracking_uri)
    while not stop.is_set():
        try:
            event = work_queue.get(timeout=0.5)
        except queue.Empty:
            continue
        if event is None:
            break
        event_id = event["event_id"]
        if event_id in _state["processed"]:
            _ack(event_id, event["run_key"], "duplicate")
            continue
        try:
            run_id = _dispatch(client, event)
            _state["processed"].add(event_id)
            _ack(event_id, event["run_key"], "logged", run_id=run_id)
        except Exception as exc:  # MLflow transports intentionally expose several exception families.
            _logger.exception("failed to log MLflow event %s", event_id)
            _ack(event_id, event["run_key"], "failed", str(exc))


def _dispatch(client, event: dict[str, Any]) -> str:
    run_id = _ensure_run(client, event)
    kind = event["event"]
    if kind == "run-start":
        for key, value in event.get("params", {}).items():
            client.log_param(run_id, str(key), str(value))
        for key, value in event.get("tags", {}).items():
            client.set_tag(run_id, str(key), str(value))
    elif kind == "metrics":
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
    run_key = event["run_key"]
    existing = _state["runs"].get(run_key)
    if existing:
        return existing
    experiment_name = event.get("experiment") or os.environ.get("MLFLOW_DEFAULT_EXPERIMENT", "edge-toolkit")
    experiment = client.get_experiment_by_name(experiment_name)
    experiment_id = experiment.experiment_id if experiment is not None else client.create_experiment(experiment_name)
    tags = {
        "edge_toolkit.run_key": run_key,
        "mlflow.runName": event.get("run_name", run_key),
    }
    run = client.create_run(experiment_id, tags=tags)
    _state["runs"][run_key] = run.info.run_id
    return run.info.run_id


def _log_metrics(client, run_id: str, metrics: dict[str, Any], step: int) -> None:
    timestamp = int(time.time() * 1000)
    for key, value in metrics.items():
        client.log_metric(run_id, str(key), float(value), timestamp=timestamp, step=step)


def _log_artifact(client, run_id: str, artifact: dict[str, Any]) -> None:
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


def _ack(
    event_id: str,
    run_key: str,
    status: str,
    message: str | None = None,
    *,
    run_id: str | None = None,
) -> None:
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
    if not isinstance(value, dict) or not value:
        raise ValueError("metrics must be a non-empty object")
    for key, metric in value.items():
        _safe_string(str(key), "metric name")
        if isinstance(metric, bool) or not isinstance(metric, (int, float)) or not math.isfinite(float(metric)):
            raise ValueError(f"metric {key!r} must be finite and numeric")


def _validate_mapping(value: Any, name: str) -> None:
    if not isinstance(value, dict):
        raise TypeError(f"{name} must be an object")
    for key, item in value.items():
        _safe_string(str(key), f"{name} key")
        if not isinstance(item, (str, int, float, bool)) or (isinstance(item, float) and not math.isfinite(item)):
            raise ValueError(f"{name} values must be finite scalars")


def _required_safe_string(value: dict[str, Any], key: str) -> str:
    if key not in value:
        raise KeyError(f"missing {key}")
    return _safe_string(value[key], key)


def _safe_string(value: Any, name: str) -> str:
    if not isinstance(value, str) or not SAFE_NAME.fullmatch(value) or ".." in value:
        raise ValueError(f"invalid {name}")
    return value


def _positive_int_env(name: str, default: int) -> int:
    value = int(os.environ.get(name, str(default)))
    return value if value > 0 else default
