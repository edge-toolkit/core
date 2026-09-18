"""Three-client multimodal vessel federated-learning demo for et-ws-pyo3-runner.

One source file supports an immutable ``coordinator`` or ``client`` role selected
by ``VESSEL_FL_ROLE`` at process startup. Three client processes train the same
small PyTorch audio/image late-fusion classifier on the prepared Croatia dataset.
Only model parameters and metrics cross the hub; client manifests and features
stay on the local filesystem. The coordinator performs sample-weighted FedAvg,
evaluates each global model, and emits generic ``mlflow-event`` frames.
"""

from __future__ import annotations

import ast
import csv
import io
import json
import logging
import os
import queue
import random
import struct
import threading
import time
from pathlib import Path
from typing import Any

_logger = logging.getLogger(__name__)

ROUND_TYPE = "vessel-fl-round"
UPDATE_TYPE = "vessel-fl-update"
COMPLETE_TYPE = "vessel-fl-complete"
COMMAND_TYPE = "vessel-fl-command"
STATUS_TYPE = "vessel-fl-status"
CLIENT_STATUS_TYPE = "vessel-fl-client-status"
COORDINATOR_STATUS_TYPE = "vessel-fl-coordinator-status"
MLFLOW_TYPE = "mlflow-event"
SCHEMA_VERSION = 1
DEFAULT_DATA_ROOT = "~/Datasets/croatia-postprocessed/croatia_1g_logmel_images"
DEFAULT_CLIENTS = ("vessel_1", "vessel_2", "vessel_3")

_state: dict[str, Any] = {
    "send": None,
    "storage": None,
    "agent_id": None,
    "role": None,
    "client_id": None,
    "stop": threading.Event(),
    "start": threading.Event(),
    "start_lock": threading.Lock(),
    "started": False,
    "client_status": None,
    "coordinator_status": None,
    "worker": None,
    "seen_rounds": set(),
    "update_queue": queue.Queue(),
}


def init(send, storage) -> None:
    """Capture runner handles and freeze this process's role."""
    role = os.environ.get("VESSEL_FL_ROLE", "client").strip().lower()
    if role not in {"client", "coordinator"}:
        raise RuntimeError("VESSEL_FL_ROLE must be 'client' or 'coordinator'")
    client_id = os.environ.get("VESSEL_FL_CLIENT_ID", "").strip()
    if role == "client" and client_id not in DEFAULT_CLIENTS:
        raise RuntimeError(f"VESSEL_FL_CLIENT_ID must be one of {DEFAULT_CLIENTS!r}")
    _state["send"] = send
    _state["storage"] = storage
    _state["role"] = role
    _state["client_id"] = client_id or None
    _logger.info("vessel FL module initialised with immutable role=%s client_id=%s", role, client_id or "-")


def on_connect(agent_id: str) -> None:
    """Start the role-specific background worker after hub registration."""
    _state["agent_id"] = agent_id
    target = _coordinator_main if _state["role"] == "coordinator" else _client_main
    worker = threading.Thread(target=target, name=f"vessel-fl-{_state['role']}", daemon=True)
    _state["worker"] = worker
    worker.start()
    _logger.info("vessel FL %s connected as %s", _state["role"], agent_id)


def on_text_frame(text: str) -> None:
    """Route protocol messages; long-running work remains off the dispatch thread."""
    try:
        message = json.loads(text)
    except (TypeError, ValueError):
        return
    if not isinstance(message, dict) or message.get("schema_version") != SCHEMA_VERSION:
        return
    if _state["role"] == "client":
        if message.get("type") == ROUND_TYPE:
            round_number = message.get("round")
            if not isinstance(round_number, int) or round_number < 0 or round_number in _state["seen_rounds"]:
                return
            _state["seen_rounds"].add(round_number)
            _state["update_queue"].put(message)
        elif message.get("type") == COMMAND_TYPE:
            _handle_status_request(message)
    elif _state["role"] == "coordinator":
        if message.get("type") == UPDATE_TYPE:
            _state["update_queue"].put(message)
        elif message.get("type") == COMMAND_TYPE:
            _handle_command(message)


def on_shutdown() -> None:
    """Stop background loops and give their current operation a brief grace period."""
    _state["stop"].set()
    _state["start"].set()
    _state["update_queue"].put(None)
    worker = _state.get("worker")
    if worker is not None:
        worker.join(timeout=10.0)


def _data_root() -> Path:
    root = Path(os.environ.get("VESSEL_FL_DATA_ROOT", DEFAULT_DATA_ROOT)).expanduser().resolve()
    required = ("feature_manifest.csv", "class_summary.csv", "clients")
    missing = [name for name in required if not (root / name).exists()]
    if missing:
        raise RuntimeError(f"dataset root {root} is missing {', '.join(missing)}")
    return root


def _handle_command(message: dict[str, Any]) -> None:
    """Accept one browser start command for this configured job."""
    if message.get("action") == "status":
        _handle_status_request(message)
        return
    if message.get("action") != "start":
        return
    job_id = os.environ.get("VESSEL_FL_JOB_ID", "vessel-fl-demo")
    if message.get("job_id") != job_id:
        return
    with _state["start_lock"]:
        if _state["started"]:
            status = "already-started"
        else:
            _state["started"] = True
            _state["start"].set()
            status = "accepted"
    _send_status(job_id, status)


def _send_status(job_id: str, status: str, **fields) -> None:
    _send_json(
        {
            "type": STATUS_TYPE,
            "schema_version": SCHEMA_VERSION,
            "job_id": job_id,
            "status": status,
            **fields,
        }
    )


def _handle_status_request(message: dict[str, Any]) -> None:
    """Replay current role status to a browser that connected after startup."""
    if message.get("action") != "status":
        return
    if message.get("job_id") != os.environ.get("VESSEL_FL_JOB_ID", "vessel-fl-demo"):
        return
    if _state["role"] == "client":
        if message.get("client_id") != _state["client_id"]:
            return
        status = _state.get("client_status")
    else:
        if message.get("role") != "coordinator":
            return
        status = _state.get("coordinator_status")
    if status:
        _send_json(status)


def _send_client_status(status: str, **fields) -> None:
    value = {
        "type": CLIENT_STATUS_TYPE,
        "schema_version": SCHEMA_VERSION,
        "job_id": os.environ.get("VESSEL_FL_JOB_ID", "vessel-fl-demo"),
        "client_id": _state["client_id"],
        "status": status,
        **fields,
    }
    _state["client_status"] = value
    _send_json(value)


def _send_coordinator_status(status: str, **fields) -> None:
    value = {
        "type": COORDINATOR_STATUS_TYPE,
        "schema_version": SCHEMA_VERSION,
        "job_id": os.environ.get("VESSEL_FL_JOB_ID", "vessel-fl-demo"),
        "status": status,
        **fields,
    }
    _state["coordinator_status"] = value
    _send_json(value)


def _bool_env(name: str, default: bool = False) -> bool:
    value = os.environ.get(name)
    if value is None:
        return default
    return value.strip().lower() in {"1", "true", "yes", "on"}


def _import_torch():
    try:
        import torch  # noqa: PLC0415 -- optional heavyweight dependency, required only by a running FL role
    except ImportError as exc:
        raise RuntimeError("PyTorch is required; run this demo with MISE_ENV=python and install pipx:torch") from exc
    return torch


def _labels(root: Path) -> list[str]:
    with (root / "class_summary.csv").open(newline="", encoding="utf-8") as source:
        labels = [row["standard_label"] for row in csv.DictReader(source) if int(row["segments"]) > 0]
    if len(labels) < 2:
        raise RuntimeError("the dataset must contain at least two classes")
    return labels


def _feature_index(root: Path) -> dict[str, str]:
    with (root / "feature_manifest.csv").open(newline="", encoding="utf-8") as source:
        return {row["segment_id"]: row["feature_path"] for row in csv.DictReader(source)}


def _load_rows(root: Path, manifest: Path, *, require_image: bool = True) -> list[dict[str, str]]:
    features = _feature_index(root)
    rows: list[dict[str, str]] = []
    with manifest.open(newline="", encoding="utf-8") as source:
        for row in csv.DictReader(source):
            feature_path = features.get(row["segment_id"])
            image_path = row.get("camera_image_output_path", "")
            if not feature_path or (require_image and not image_path):
                continue
            feature = root / feature_path
            image = root / image_path if image_path else None
            if feature.is_file() and (not require_image or (image is not None and image.is_file())):
                rows.append(
                    {
                        "segment_id": row["segment_id"],
                        "label": row["standard_label"],
                        "feature": str(feature),
                        "image": str(image) if image is not None else "",
                    }
                )
    if not rows:
        raise RuntimeError(f"manifest {manifest} has no usable paired samples")
    return rows


def _read_npy(path: str, torch):
    """Read the pipeline's little-endian float32 NPY without a NumPy runtime dependency."""
    data = Path(path).read_bytes()
    if data[:6] != b"\x93NUMPY":
        raise ValueError(f"{path} is not an NPY file")
    major = data[6]
    if major == 1:
        header_length = struct.unpack_from("<H", data, 8)[0]
        header_start = 10
    elif major in {2, 3}:
        header_length = struct.unpack_from("<I", data, 8)[0]
        header_start = 12
    else:
        raise ValueError(f"unsupported NPY version {major}")
    header = ast.literal_eval(data[header_start : header_start + header_length].decode("latin1").strip())
    if header["descr"] not in {"<f4", "=f4"} or header["fortran_order"]:
        raise ValueError("expected row-major little-endian float32 NPY")
    shape = tuple(int(item) for item in header["shape"])
    payload = bytearray(data[header_start + header_length :])
    tensor = torch.frombuffer(payload, dtype=torch.float32).clone().reshape(shape)
    if tensor.ndim != 2:
        raise ValueError(f"expected a two-dimensional log-mel tensor, got {shape}")
    tensor = (tensor - tensor.mean()) / tensor.std().clamp_min(1e-6)
    return torch.nn.functional.adaptive_avg_pool2d(tensor[None, None], (32, 32))[0]


def _read_image(path: str, torch):
    try:
        from PIL import Image  # noqa: PLC0415 -- optional runtime dependency used only during materialisation
    except ImportError as exc:
        raise RuntimeError("Pillow is required by the vessel demo image branch") from exc
    with Image.open(path) as source:
        image = source.convert("RGB").resize((64, 64))
        raw = bytearray(image.tobytes())
    return torch.frombuffer(raw, dtype=torch.uint8).clone().reshape(64, 64, 3).permute(2, 0, 1).float() / 255.0


def _new_model(torch, class_count: int):
    class VesselFusionModel(torch.nn.Module):
        def __init__(self) -> None:
            super().__init__()
            self.audio = torch.nn.Sequential(
                torch.nn.Conv2d(1, 8, 3, padding=1),
                torch.nn.ReLU(),
                torch.nn.MaxPool2d(2),
                torch.nn.Conv2d(8, 16, 3, padding=1),
                torch.nn.ReLU(),
                torch.nn.AdaptiveAvgPool2d((1, 1)),
                torch.nn.Flatten(),
            )
            self.image = torch.nn.Sequential(
                torch.nn.Conv2d(3, 8, 3, stride=2, padding=1),
                torch.nn.ReLU(),
                torch.nn.Conv2d(8, 16, 3, stride=2, padding=1),
                torch.nn.ReLU(),
                torch.nn.AdaptiveAvgPool2d((1, 1)),
                torch.nn.Flatten(),
            )
            self.classifier = torch.nn.Sequential(
                torch.nn.Linear(32, 32),
                torch.nn.ReLU(),
                torch.nn.Dropout(0.15),
                torch.nn.Linear(32, class_count),
            )

        def forward(self, audio, image):
            return self.classifier(torch.cat((self.audio(audio), self.image(image)), dim=1))

    return VesselFusionModel()


def _batches(rows: list[dict[str, str]], batch_size: int, seed: int, *, shuffle: bool):
    indices = list(range(len(rows)))
    if shuffle:
        random.Random(seed).shuffle(indices)
    for start in range(0, len(indices), batch_size):
        yield [rows[index] for index in indices[start : start + batch_size]]


def _materialize(batch: list[dict[str, str]], labels: list[str], torch):
    label_index = {label: index for index, label in enumerate(labels)}
    audio = torch.stack([_read_npy(row["feature"], torch) for row in batch])
    images = torch.stack([_read_image(row["image"], torch) for row in batch])
    targets = torch.tensor([label_index[row["label"]] for row in batch], dtype=torch.long)
    return audio, images, targets


def _train_local(model, rows: list[dict[str, str]], labels: list[str], round_number: int, torch) -> dict[str, float]:
    model.train()
    optimizer = torch.optim.SGD(
        model.parameters(),
        lr=float(os.environ.get("VESSEL_FL_LEARNING_RATE", "0.03")),
        momentum=0.9,
    )
    epochs = _positive_int_env("VESSEL_FL_LOCAL_EPOCHS", 1)
    batch_size = _positive_int_env("VESSEL_FL_BATCH_SIZE", 16)
    total_loss = 0.0
    total = 0
    correct = 0
    for epoch in range(epochs):
        for batch in _batches(rows, batch_size, 10_000 * round_number + epoch, shuffle=True):
            audio, images, targets = _materialize(batch, labels, torch)
            optimizer.zero_grad(set_to_none=True)
            logits = model(audio, images)
            loss = torch.nn.functional.cross_entropy(logits, targets)
            loss.backward()
            optimizer.step()
            total_loss += float(loss.item()) * len(batch)
            correct += int((logits.argmax(dim=1) == targets).sum().item())
            total += len(batch)
    return {"train_loss": total_loss / total, "train_accuracy": correct / total}


def _evaluate(model, rows: list[dict[str, str]], labels: list[str], torch) -> dict[str, float]:
    model.eval()
    total_loss = 0.0
    total = 0
    correct = 0
    confusion = [[0 for _ in labels] for _ in labels]
    with torch.no_grad():
        for batch in _batches(rows, _positive_int_env("VESSEL_FL_BATCH_SIZE", 16), 0, shuffle=False):
            audio, images, targets = _materialize(batch, labels, torch)
            logits = model(audio, images)
            total_loss += float(torch.nn.functional.cross_entropy(logits, targets).item()) * len(batch)
            predictions = logits.argmax(dim=1)
            correct += int((predictions == targets).sum().item())
            total += len(batch)
            for target, prediction in zip(targets.tolist(), predictions.tolist(), strict=True):
                confusion[target][prediction] += 1
    f1_values = []
    for index in range(len(labels)):
        true_positive = confusion[index][index]
        false_positive = sum(confusion[row][index] for row in range(len(labels)) if row != index)
        false_negative = sum(confusion[index][column] for column in range(len(labels)) if column != index)
        denominator = 2 * true_positive + false_positive + false_negative
        f1_values.append(0.0 if denominator == 0 else 2 * true_positive / denominator)
    return {
        "loss": total_loss / total,
        "accuracy": correct / total,
        "macro_f1": sum(f1_values) / len(f1_values),
    }


def _checkpoint_bytes(model, labels: list[str], round_number: int, torch) -> bytes:
    buffer = io.BytesIO()
    torch.save(
        {
            "schema_version": SCHEMA_VERSION,
            "round": round_number,
            "labels": labels,
            "state_dict": {name: tensor.detach().cpu() for name, tensor in model.state_dict().items()},
        },
        buffer,
    )
    return buffer.getvalue()


def _load_checkpoint(payload: bytes, torch) -> dict[str, Any]:
    checkpoint = torch.load(io.BytesIO(payload), map_location="cpu", weights_only=True)
    if checkpoint.get("schema_version") != SCHEMA_VERSION or not isinstance(checkpoint.get("state_dict"), dict):
        raise ValueError("invalid vessel FL checkpoint")
    return checkpoint


def _client_main() -> None:
    torch = _import_torch()
    torch.manual_seed(17)
    torch.set_num_threads(_positive_int_env("VESSEL_FL_TORCH_THREADS", 2))
    root = _data_root()
    client_id = _state["client_id"]
    labels = _labels(root)
    train_rows = _load_rows(root, root / "clients" / client_id / "train_manifest.csv")
    val_rows = _load_rows(root, root / "clients" / client_id / "val_manifest.csv")
    _logger.info("%s loaded %d train and %d validation pairs", client_id, len(train_rows), len(val_rows))
    _send_client_status("dataset-loaded", train_samples=len(train_rows), validation_samples=len(val_rows))
    while not _state["stop"].is_set():
        message = _state["update_queue"].get()
        if message is None:
            return
        try:
            _run_client_round(message, train_rows, val_rows, labels, torch)
        except Exception as exc:  # A failed local round is reported without killing the long-lived agent.
            _logger.exception("%s failed round %s", client_id, message.get("round"))
            _send_client_status("failed", round=message.get("round"), error=str(exc)[:500])
            _send_json(
                {
                    "type": UPDATE_TYPE,
                    "schema_version": SCHEMA_VERSION,
                    "job_id": message.get("job_id"),
                    "round": message.get("round"),
                    "client_id": client_id,
                    "status": "failed",
                    "error": str(exc)[:500],
                }
            )


def _run_client_round(message, train_rows, val_rows, labels, torch) -> None:
    expected_job = os.environ.get("VESSEL_FL_JOB_ID", "vessel-fl-demo")
    if message.get("job_id") != expected_job:
        raise ValueError("unexpected FL job")
    coordinator = message.get("coordinator_id")
    filename = message.get("filename")
    if not isinstance(coordinator, str) or not isinstance(filename, str):
        raise TypeError("round message has no checkpoint pointer")
    payload = _state["storage"].get(coordinator, filename)
    if payload is None:
        raise RuntimeError(f"global checkpoint {coordinator}/{filename} was not found")
    checkpoint = _load_checkpoint(bytes(payload), torch)
    if checkpoint["labels"] != labels or checkpoint["round"] != message["round"]:
        raise ValueError("global checkpoint metadata does not match this client")
    model = _new_model(torch, len(labels))
    model.load_state_dict(checkpoint["state_dict"], strict=True)
    _send_client_status(
        "training",
        round=message["round"],
        train_samples=len(train_rows),
        validation_samples=len(val_rows),
    )
    train_metrics = _train_local(model, train_rows, labels, message["round"], torch)
    validation = _evaluate(model, val_rows, labels, torch)
    update_name = f"{message['job_id']}-round-{message['round']:03d}-{_state['client_id']}.pt"
    _send_client_status("uploading-update", round=message["round"])
    _state["storage"].put(update_name, _checkpoint_bytes(model, labels, message["round"], torch))
    metrics = {
        **train_metrics,
        "validation_loss": validation["loss"],
        "validation_accuracy": validation["accuracy"],
        "validation_macro_f1": validation["macro_f1"],
    }
    _send_json(
        {
            "type": UPDATE_TYPE,
            "schema_version": SCHEMA_VERSION,
            "job_id": message["job_id"],
            "round": message["round"],
            "client_id": _state["client_id"],
            "status": "ready",
            "bucket": _state["agent_id"],
            "filename": update_name,
            "samples": len(train_rows),
            "metrics": metrics,
        }
    )
    _send_client_status(
        "waiting",
        round=message["round"],
        samples=len(train_rows),
        metrics=metrics,
    )
    _logger.info("%s completed round %d: %s", _state["client_id"], message["round"], metrics)


def _coordinator_main() -> None:
    job_id = os.environ.get("VESSEL_FL_JOB_ID", "vessel-fl-demo")
    try:
        if _bool_env("VESSEL_FL_AUTO_START"):
            time.sleep(float(os.environ.get("VESSEL_FL_START_DELAY_SECONDS", "3")))
            with _state["start_lock"]:
                _state["started"] = True
                _state["start"].set()
            _logger.info("coordinator auto-start enabled")
        else:
            _logger.info("coordinator waiting for a browser start command for job %s", job_id)
            _send_coordinator_status("waiting-for-start", expected_clients=len(DEFAULT_CLIENTS))
        while not _state["start"].wait(timeout=0.5):
            if _state["stop"].is_set():
                return
        if _state["stop"].is_set():
            return
        _send_status(job_id, "running")
        _send_coordinator_status("run-started")

        torch = _import_torch()
        torch.manual_seed(17)
        torch.set_num_threads(_positive_int_env("VESSEL_FL_TORCH_THREADS", 2))
        root = _data_root()
        labels = _labels(root)
        test_rows = _load_rows(root, root / "test_manifest.csv")
        model = _new_model(torch, len(labels))
        clients = tuple(
            item.strip() for item in os.environ.get("VESSEL_FL_CLIENTS", ",".join(DEFAULT_CLIENTS)).split(",")
        )
        rounds = _positive_int_env("VESSEL_FL_ROUNDS", 3)
        _emit_mlflow(
            "run-start",
            f"{job_id}:start",
            job_id,
            params={
                "clients": len(clients),
                "rounds": rounds,
                "local_epochs": _positive_int_env("VESSEL_FL_LOCAL_EPOCHS", 1),
                "aggregation": "fedavg",
                "modalities": "log_mel,camera_image",
                "classes": len(labels),
            },
            tags={"application": "vessel-classification", "runner": "pyo3"},
        )
        for round_number in range(rounds):
            _send_coordinator_status(
                "round-started",
                round=round_number,
                expected_clients=len(clients),
                received_clients=[],
            )
            checkpoint_name = f"{job_id}-global-round-{round_number:03d}.pt"
            _state["storage"].put(checkpoint_name, _checkpoint_bytes(model, labels, round_number, torch))
            updates = _collect_round(job_id, round_number, checkpoint_name, clients)
            _send_coordinator_status(
                "aggregating",
                round=round_number,
                expected_clients=len(clients),
                received_clients=[update["client_id"] for update in updates],
            )
            _fed_avg(model, updates, torch)
            _send_coordinator_status("evaluating", round=round_number)
            evaluation = _evaluate(model, test_rows, labels, torch)
            client_accuracy = sum(update["metrics"]["validation_accuracy"] for update in updates) / len(updates)
            metrics = {
                "global_test_loss": evaluation["loss"],
                "global_test_accuracy": evaluation["accuracy"],
                "global_test_macro_f1": evaluation["macro_f1"],
                "mean_client_validation_accuracy": client_accuracy,
                "participating_clients": float(len(updates)),
            }
            _emit_mlflow("metrics", f"{job_id}:round:{round_number}", job_id, step=round_number, metrics=metrics)
            _send_coordinator_status("round-completed", round=round_number, metrics=metrics)
            _logger.info("coordinator completed round %d: %s", round_number, metrics)
        final_name = f"{job_id}-final-model.pt"
        _state["storage"].put(final_name, _checkpoint_bytes(model, labels, rounds, torch))
        summary_name = f"{job_id}-summary.json"
        summary = {
            "job_id": job_id,
            "rounds": rounds,
            "clients": clients,
            "labels": labels,
            "test_samples": len(test_rows),
            "metrics": evaluation,
        }
        _state["storage"].put(summary_name, json.dumps(summary, indent=2, sort_keys=True).encode())
        artifacts = [
            {"bucket": _state["agent_id"], "filename": final_name, "artifact_path": "model/final-model.pt"},
            {"bucket": _state["agent_id"], "filename": summary_name, "artifact_path": "evaluation/summary.json"},
        ]
        _emit_mlflow("artifacts", f"{job_id}:artifacts", job_id, artifacts=artifacts)
        _emit_mlflow("run-end", f"{job_id}:finish", job_id, step=rounds, status="FINISHED", metrics=evaluation)
        _send_json(
            {
                "type": COMPLETE_TYPE,
                "schema_version": SCHEMA_VERSION,
                "job_id": job_id,
                "bucket": _state["agent_id"],
                "filename": final_name,
                "summary": summary,
            }
        )
        _send_status(job_id, "completed", metrics=evaluation)
        _send_coordinator_status("run-completed", metrics=evaluation)
    except Exception:
        _logger.exception("vessel FL coordinator failed")
        _send_status(job_id, "failed")
        _send_coordinator_status("failed")
        _emit_mlflow("run-end", f"{job_id}:failed", job_id, status="FAILED")


def _collect_round(job_id: str, round_number: int, checkpoint_name: str, clients: tuple[str, ...]):
    deadline = time.monotonic() + float(os.environ.get("VESSEL_FL_ROUND_TIMEOUT_SECONDS", "180"))
    updates: dict[str, dict[str, Any]] = {}
    last_broadcast = 0.0
    while len(updates) < len(clients):
        now = time.monotonic()
        if now >= deadline:
            missing = sorted(set(clients) - set(updates))
            raise TimeoutError(f"round {round_number} timed out waiting for {missing}")
        if now - last_broadcast >= 2.0:
            _send_json(
                {
                    "type": ROUND_TYPE,
                    "schema_version": SCHEMA_VERSION,
                    "job_id": job_id,
                    "round": round_number,
                    "coordinator_id": _state["agent_id"],
                    "filename": checkpoint_name,
                }
            )
            last_broadcast = now
        try:
            message = _state["update_queue"].get(timeout=min(0.5, deadline - now))
        except queue.Empty:
            continue
        if message is None:
            raise RuntimeError("coordinator stopped")
        client_id = message.get("client_id")
        if (
            message.get("type") != UPDATE_TYPE
            or message.get("job_id") != job_id
            or message.get("round") != round_number
            or client_id not in clients
        ):
            continue
        if message.get("status") != "ready":
            raise RuntimeError(f"{client_id} failed round {round_number}: {message.get('error', 'unknown error')}")
        if client_id in updates:
            continue
        payload = _state["storage"].get(message["bucket"], message["filename"])
        if payload is None:
            continue
        message["checkpoint"] = _load_checkpoint(bytes(payload), _import_torch())
        updates[client_id] = message
        _send_coordinator_status(
            "waiting-for-clients",
            round=round_number,
            expected_clients=len(clients),
            received_clients=sorted(updates),
        )
    return [updates[client] for client in clients]


def _fed_avg(model, updates: list[dict[str, Any]], torch) -> None:
    total_samples = sum(int(update["samples"]) for update in updates)
    if total_samples <= 0:
        raise ValueError("FedAvg received no samples")
    averaged = {}
    reference = model.state_dict()
    for name, reference_tensor in reference.items():
        tensors = [update["checkpoint"]["state_dict"][name] for update in updates]
        if reference_tensor.is_floating_point():
            value = torch.zeros_like(reference_tensor)
            for update, tensor in zip(updates, tensors, strict=True):
                value.add_(tensor.to(value.dtype), alpha=int(update["samples"]) / total_samples)
            averaged[name] = value
        else:
            averaged[name] = tensors[0]
    model.load_state_dict(averaged, strict=True)


def _emit_mlflow(event: str, event_id: str, run_key: str, **fields) -> None:
    value = {
        "type": MLFLOW_TYPE,
        "schema_version": SCHEMA_VERSION,
        "event_id": event_id,
        "event": event,
        "run_key": run_key,
        "experiment": os.environ.get("VESSEL_FL_MLFLOW_EXPERIMENT", "vessel-multimodal-fl"),
        "run_name": os.environ.get("VESSEL_FL_MLFLOW_RUN_NAME", run_key),
        **fields,
    }
    _send_json(value)


def _send_json(value: dict[str, Any]) -> None:
    _state["send"].text(json.dumps(value, separators=(",", ":"), sort_keys=True))


def _positive_int_env(name: str, default: int) -> int:
    value = int(os.environ.get(name, str(default)))
    return value if value > 0 else default
