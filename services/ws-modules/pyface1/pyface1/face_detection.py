"""RetinaFace post-processing helpers for the pyface1 workflow."""

from __future__ import annotations

import math
import json
import time
from datetime import datetime
from functools import lru_cache
from typing import Iterable, Sequence, TypedDict

FACE_MODEL_PATH = "/modules/et-model-face1/video_cv.onnx"
FACE_INPUT_WIDTH = 640
FACE_INPUT_HEIGHT = 608
FACE_INFERENCE_INTERVAL_MS = 750
FACE_RENDER_INTERVAL_MS = 60
FACE_MAX_INFERENCES = 20
FACE_MAX_RUNTIME_MS = 30_000
RETINAFACE_CONFIDENCE_THRESHOLD = 0.75
RETINAFACE_NMS_THRESHOLD = 0.4
RETINAFACE_VARIANCES = (0.1, 0.2)
RETINAFACE_MIN_SIZES = ((16.0, 32.0), (64.0, 128.0), (256.0, 512.0))
RETINAFACE_STEPS = (8.0, 16.0, 32.0)


Box = list[float]
Prior = tuple[float, float, float, float]
DecodedBox = tuple[float, float, float, float]


class Detection(TypedDict):
    label: str
    class_index: int
    score: float
    box: Box


class DetectionSummary(TypedDict):
    detections: list[Detection]
    confidence: float
    processed_at: str


async def run(
    input_name,
    output_names,
    infer_once,
    send_event,
    render,
    sleep_ms,
    log,
    set_status,
    should_stop,
) -> None:
    """Run the browser face detection workflow using JS platform callbacks."""
    output_names = [str(name) for name in output_names]
    last_has_detection = False
    inference_count = 0
    started_at = time.monotonic()
    detections: list[Detection] = []

    startup_summary = initial_summary()
    set_status(status_text(str(input_name), output_names, startup_summary))

    while not should_stop():
        elapsed_ms = (time.monotonic() - started_at) * 1000.0
        if inference_count >= FACE_MAX_INFERENCES or elapsed_ms >= FACE_MAX_RUNTIME_MS:
            break

        try:
            capture = await infer_once()
            summary = decode_outputs(
                capture["loc"],
                capture["conf"],
                capture["landm"],
                capture["resize_ratio"],
                capture["source_width"],
                capture["source_height"],
            )
            inference_count += 1
            detections = summary["detections"]
            has_detection = bool(detections)
            changed = last_has_detection != has_detection
            last_has_detection = has_detection

            set_status(status_text(str(input_name), output_names, summary))
            render(detections_json(detections))
            send_event(
                client_event_json(
                    event_payload(
                        summary,
                        changed,
                        str(input_name),
                        output_names,
                        capture["source_width"],
                        capture["source_height"],
                    )
                )
            )
        except Exception as exc:
            message = f"pyface1 face detection: inference error\n{exc}"
            set_status(message)
            log(f"inference error: {exc}")

        remaining_ms = FACE_INFERENCE_INTERVAL_MS
        while remaining_ms > 0 and not should_stop():
            render(detections_json(detections))
            delay = min(FACE_RENDER_INTERVAL_MS, remaining_ms)
            await sleep_ms(delay)
            remaining_ms -= delay

    if inference_count >= FACE_MAX_INFERENCES:
        log(f"workflow finished automatically after {FACE_MAX_INFERENCES} inferences")
    elif (time.monotonic() - started_at) * 1000.0 >= FACE_MAX_RUNTIME_MS:
        log("workflow finished automatically after 30 seconds")
    set_status(stopped_status())


def config() -> dict[str, object]:
    """Return browser-facing constants for this workflow."""
    return {
        "model_path": FACE_MODEL_PATH,
        "input_width": FACE_INPUT_WIDTH,
        "input_height": FACE_INPUT_HEIGHT,
    }


def starting_status() -> str:
    return "pyface1 face detection: starting"


def stopped_status() -> str:
    return "pyface1 face detection demo stopped."


def model_log_message() -> str:
    return f"loading RetinaFace model from {FACE_MODEL_PATH}"


def validate_output_names(output_names: Iterable[object]) -> list[str]:
    output_names = [str(name) for name in output_names]
    if len(output_names) < 3:
        raise ValueError("RetinaFace session did not expose the expected outputs")
    return output_names


def initial_summary() -> DetectionSummary:
    return {
        "detections": [],
        "confidence": 0.0,
        "processed_at": "waiting for first inference",
    }


def preprocess_geometry(source_width: float, source_height: float) -> dict[str, float]:
    source_width = require_positive_finite(source_width, "source_width")
    source_height = require_positive_finite(source_height, "source_height")
    target_ratio = FACE_INPUT_HEIGHT / FACE_INPUT_WIDTH
    resize_ratio = (
        FACE_INPUT_WIDTH / source_width
        if source_height / source_width <= target_ratio
        else FACE_INPUT_HEIGHT / source_height
    )
    return {
        "resize_ratio": resize_ratio,
        "resized_width": float(
            int(clamp(round(source_width * resize_ratio), 1, FACE_INPUT_WIDTH))
        ),
        "resized_height": float(
            int(clamp(round(source_height * resize_ratio), 1, FACE_INPUT_HEIGHT))
        ),
    }


def detections_json(detections: list[Detection]) -> str:
    return json.dumps(detections)


def client_event_json(details: dict[str, object]) -> str:
    return json.dumps(
        {
            "type": "client_event",
            "capability": "face_detection",
            "action": "inference",
            "details": details,
        }
    )


def decode_outputs(
    loc_values: Iterable[object],
    conf_values: Iterable[object],
    landm_values: Iterable[object],
    resize_ratio: float,
    source_width: float,
    source_height: float,
) -> DetectionSummary:
    """Decode RetinaFace ONNX outputs into detections and summary metadata."""
    resize_ratio = require_positive_finite(resize_ratio, "resize_ratio")
    source_width = require_non_negative_finite(source_width, "source_width")
    source_height = require_non_negative_finite(source_height, "source_height")

    loc = output_values(loc_values, "loc", 4)
    conf = output_values(conf_values, "conf", 2)
    landm = output_values(landm_values, "landm", 10)
    prior_count = len(loc) // 4

    if (
        prior_count == 0
        or len(conf) != prior_count * 2
        or len(landm) != prior_count * 10
    ):
        raise ValueError("RetinaFace outputs had unexpected shapes")

    priors = model_priors()
    if len(priors) != prior_count:
        raise ValueError("RetinaFace priors did not match output count")

    detections: list[Detection] = []
    for index in range(prior_count):
        score = softmax((conf[index * 2], conf[index * 2 + 1]))[1]
        if score < RETINAFACE_CONFIDENCE_THRESHOLD:
            continue

        decoded = decode_box(
            (
                loc[index * 4],
                loc[index * 4 + 1],
                loc[index * 4 + 2],
                loc[index * 4 + 3],
            ),
            priors[index],
        )
        box: Box = [
            clamp((decoded[0] * FACE_INPUT_WIDTH) / resize_ratio, 0.0, source_width),
            clamp((decoded[1] * FACE_INPUT_HEIGHT) / resize_ratio, 0.0, source_height),
            clamp((decoded[2] * FACE_INPUT_WIDTH) / resize_ratio, 0.0, source_width),
            clamp((decoded[3] * FACE_INPUT_HEIGHT) / resize_ratio, 0.0, source_height),
        ]

        detections.append(
            {
                "label": "face",
                "class_index": 0,
                "score": score,
                "box": box,
            }
        )

    detections = apply_nms(detections, RETINAFACE_NMS_THRESHOLD)
    confidence = detections[0]["score"] if detections else 0.0
    return {
        "detections": detections,
        "confidence": float(confidence),
        "processed_at": datetime.now().strftime("%X"),
    }


def status_text(
    input_name: str, output_names: Iterable[object], summary: DetectionSummary
) -> str:
    """Render the browser status text used by the face detection demo."""
    outputs = ", ".join(str(name) for name in output_names)
    lines = [
        "pyface1 face detection demo",
        f"model file: {FACE_MODEL_PATH}",
        f"input: {input_name}",
        f"outputs: {outputs}",
        f"detections: {len(summary['detections'])}",
        f"best confidence: {summary['confidence']:.4f}",
        f"processed at: {summary['processed_at']}",
    ]

    if summary["detections"]:
        box = summary["detections"][0]["box"]
        lines.extend(
            [
                "",
                f"best box: {box[0]:.1f}, {box[1]:.1f}, {box[2]:.1f}, {box[3]:.1f}",
            ]
        )

    return "\n".join(lines)


def event_payload(
    summary: DetectionSummary,
    changed: bool,
    input_name: str,
    output_names: Iterable[object],
    source_width: float,
    source_height: float,
) -> dict[str, object]:
    """Build the WebSocket client event payload."""
    source_width = require_non_negative_finite(source_width, "source_width")
    source_height = require_non_negative_finite(source_height, "source_height")

    has_detection = bool(summary["detections"])
    return {
        "mode": "detection",
        "detected_class": "face" if has_detection else "no_detection",
        "class_index": 0 if has_detection else -1,
        "confidence": summary["confidence"],
        "detections": summary["detections"],
        "changed": changed,
        "processed_at": summary["processed_at"],
        "model_path": FACE_MODEL_PATH,
        "input_name": input_name,
        "output_names": list(output_names),
        "source_resolution": {
            "width": float(source_width),
            "height": float(source_height),
        },
    }


def build_priors(image_height: float, image_width: float) -> list[Prior]:
    image_height = require_positive_finite(image_height, "image_height")
    image_width = require_positive_finite(image_width, "image_width")

    priors: list[Prior] = []
    for index, step in enumerate(RETINAFACE_STEPS):
        feature_map_height = math.ceil(image_height / step)
        feature_map_width = math.ceil(image_width / step)
        for row in range(feature_map_height):
            for column in range(feature_map_width):
                for min_size in RETINAFACE_MIN_SIZES[index]:
                    priors.append(
                        (
                            ((column + 0.5) * step) / image_width,
                            ((row + 0.5) * step) / image_height,
                            min_size / image_width,
                            min_size / image_height,
                        )
                    )
    return priors


@lru_cache(maxsize=1)
def model_priors() -> tuple[Prior, ...]:
    return tuple(build_priors(float(FACE_INPUT_HEIGHT), float(FACE_INPUT_WIDTH)))


def decode_box(loc: Sequence[float], prior: Sequence[float]) -> DecodedBox:
    if len(loc) != 4:
        raise ValueError("loc must contain exactly 4 values")
    if len(prior) != 4:
        raise ValueError("prior must contain exactly 4 values")

    center_x = prior[0] + loc[0] * RETINAFACE_VARIANCES[0] * prior[2]
    center_y = prior[1] + loc[1] * RETINAFACE_VARIANCES[0] * prior[3]
    width = prior[2] * math.exp(loc[2] * RETINAFACE_VARIANCES[1])
    height = prior[3] * math.exp(loc[3] * RETINAFACE_VARIANCES[1])
    return (
        center_x - width / 2.0,
        center_y - height / 2.0,
        center_x + width / 2.0,
        center_y + height / 2.0,
    )


def apply_nms(detections: list[Detection], threshold: float) -> list[Detection]:
    threshold = require_non_negative_finite(threshold, "threshold")
    kept: list[Detection] = []
    for candidate in sorted(detections, key=lambda item: item["score"], reverse=True):
        if all(compute_iou(candidate, accepted) <= threshold for accepted in kept):
            kept.append(candidate)
    return kept


def compute_iou(left: Detection, right: Detection) -> float:
    left_box = left["box"]
    right_box = right["box"]
    x1 = max(left_box[0], right_box[0])
    y1 = max(left_box[1], right_box[1])
    x2 = min(left_box[2], right_box[2])
    y2 = min(left_box[3], right_box[3])
    width = max(x2 - x1 + 1.0, 0.0)
    height = max(y2 - y1 + 1.0, 0.0)
    intersection = width * height
    left_area = max(left_box[2] - left_box[0] + 1.0, 0.0) * max(
        left_box[3] - left_box[1] + 1.0,
        0.0,
    )
    right_area = max(right_box[2] - right_box[0] + 1.0, 0.0) * max(
        right_box[3] - right_box[1] + 1.0,
        0.0,
    )
    return intersection / max(left_area + right_area - intersection, 1e-6)


def softmax(values: Iterable[object]) -> list[float]:
    values = [float(value) for value in values]
    if not values:
        return []
    max_value = max(values)
    exps = [math.exp(value - max_value) for value in values]
    total = sum(exps)
    return [value / total for value in exps]


def clamp(value: float, minimum: float, maximum: float) -> float:
    return max(minimum, min(value, maximum))


def output_values(values: Iterable[object], name: str, stride: int) -> list[float]:
    if stride <= 0:
        raise ValueError("stride must be positive")

    try:
        output = [float(value) for value in values]
    except (TypeError, ValueError) as exc:
        raise ValueError(f"{name} output contained non-numeric values") from exc

    if len(output) % stride != 0:
        raise ValueError("RetinaFace outputs had unexpected shapes")
    return output


def require_positive_finite(value: float, name: str) -> float:
    value = float(value)
    if not math.isfinite(value) or value <= 0.0:
        raise ValueError(f"{name} must be a positive finite number")
    return value


def require_non_negative_finite(value: float, name: str) -> float:
    value = float(value)
    if not math.isfinite(value) or value < 0.0:
        raise ValueError(f"{name} must be a non-negative finite number")
    return value
