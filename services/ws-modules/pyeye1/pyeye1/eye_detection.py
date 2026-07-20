"""Eye bounding-box post-processing for the pyeye1 MediaPipe FaceLandmarker workflow.

The browser shim runs Google's MediaPipe FaceLandmarker (a maintained, offline `.task` bundle that internally
runs a face detector then a mesh model) and hands the resulting normalized face landmarks to this module, which
turns each eye's contour-landmark cluster into a tight bounding box in source-image pixels.
"""

from __future__ import annotations

import json
import math
import time
from collections.abc import Iterable, Sequence
from datetime import datetime
from typing import Any, TypedDict

from et_ws.messages import WsClientEvent

# Served by the MediaPipe tasks-vision runtime module and the model module (see config()).
EYE_MODEL_PATH = "/modules/et-model-eye1/face_landmarker.task"
VISION_BUNDLE_PATH = "/modules/@mediapipe/tasks-vision/vision_bundle.mjs"
VISION_WASM_PATH = "/modules/@mediapipe/tasks-vision/wasm"

# FaceLandmarker returns 478 landmarks (the 468-point mesh plus 10 iris points), each x/y normalized to [0, 1]
# against the input frame. The eye-box needs only the mesh contour indices (all < 468).
MESH_LANDMARK_COUNT = 478
CONTOUR_LANDMARK_COUNT = 468
# Eye-contour landmark indices in the MediaPipe mesh (subject's perspective, so "left" appears on the right of
# a non-mirrored frame). A tight per-eye box is the min/max over each cluster.
RIGHT_EYE_INDICES = (33, 7, 163, 144, 145, 153, 154, 155, 133, 173, 157, 158, 159, 160, 161, 246)
LEFT_EYE_INDICES = (263, 249, 390, 373, 374, 380, 381, 382, 362, 398, 384, 385, 386, 387, 388, 466)

INFERENCE_INTERVAL_MS = 750
RENDER_INTERVAL_MS = 60
MAX_INFERENCES = 20
MAX_RUNTIME_MS = 30_000


Box = list[float]


class Eye(TypedDict):
    """One detected eye: its label and bounding box in source-image pixels."""

    label: str
    box: Box


class FaceEyes(TypedDict):
    """One detected face: its overall landmark bounds and the two eye boxes within it."""

    face_box: Box
    eyes: list[Eye]


async def run(infer, send_event, render, sleep_ms, log, set_status, should_stop) -> None:
    """Run the browser eye detection workflow using JS platform callbacks.

    `infer()` returns a JSON string `{"faces": [[x0, y0, x1, y1, ...], ...], "width": W, "height": H}` where each
    face is the flat list of normalized landmark coordinates from MediaPipe FaceLandmarker.
    """
    inference_count = 0
    started_at = time.monotonic()
    results: list[FaceEyes] = []

    set_status(starting_status())

    while not should_stop():
        elapsed_ms = (time.monotonic() - started_at) * 1000.0
        if inference_count >= MAX_INFERENCES or elapsed_ms >= MAX_RUNTIME_MS:
            break

        try:
            capture = json.loads(await infer())
            results = build_results(capture["faces"], capture["width"], capture["height"])
            inference_count += 1

            set_status(status_text(results))
            render(results_json(results))
            send_event(client_event_json(event_payload(results, capture["width"], capture["height"])))
        except Exception as exc:
            message = f"pyeye1 eye detection: inference error\n{exc}"
            set_status(message)
            log(f"inference error: {exc}")

        remaining_ms = INFERENCE_INTERVAL_MS
        while remaining_ms > 0 and not should_stop():
            render(results_json(results))
            delay = min(RENDER_INTERVAL_MS, remaining_ms)
            await sleep_ms(delay)
            remaining_ms -= delay

    if inference_count >= MAX_INFERENCES:
        log(f"workflow finished automatically after {MAX_INFERENCES} inferences")
    elif (time.monotonic() - started_at) * 1000.0 >= MAX_RUNTIME_MS:
        log("workflow finished automatically after 30 seconds")
    set_status(stopped_status())


def config() -> dict[str, object]:
    """Return browser-facing constants: the model asset plus the MediaPipe runtime bundle + wasm roots."""
    return {
        "model_path": EYE_MODEL_PATH,
        "bundle_path": VISION_BUNDLE_PATH,
        "wasm_path": VISION_WASM_PATH,
    }


def starting_status() -> str:
    """Return the status line shown while the workflow starts up."""
    return "pyeye1 eye detection: starting"


def stopped_status() -> str:
    """Return the status line shown once the workflow has stopped."""
    return "pyeye1 eye detection demo stopped."


def model_log_message() -> str:
    """Return the log line emitted when loading the model."""
    return f"loading MediaPipe FaceLandmarker from {EYE_MODEL_PATH}"


def decode_eye_boxes(landmarks: Iterable[Any], image_width: float, image_height: float) -> dict[str, Box]:
    """Decode the left and right eye bounding boxes from one face's normalized FaceLandmarker landmarks."""
    values = [float(value) for value in landmarks]
    if len(values) < CONTOUR_LANDMARK_COUNT * 2:
        raise ValueError("FaceLandmarker output did not contain the expected face-mesh landmarks")

    width = require_positive_finite(image_width, "image_width")
    height = require_positive_finite(image_height, "image_height")
    return {
        "left_eye": eye_box(values, LEFT_EYE_INDICES, width, height),
        "right_eye": eye_box(values, RIGHT_EYE_INDICES, width, height),
    }


def eye_box(values: Sequence[float], indices: Sequence[int], width: float, height: float) -> Box:
    """Map one eye's contour landmarks from normalized [0, 1] coords to a source-pixel bounding box."""
    xs = [clamp(values[index * 2] * width, 0.0, width) for index in indices]
    ys = [clamp(values[index * 2 + 1] * height, 0.0, height) for index in indices]
    return [min(xs), min(ys), max(xs), max(ys)]


def face_bounds(landmarks: Sequence[float], width: float, height: float) -> Box:
    """Return the bounding box of all of a face's landmarks, in source pixels (context for the eye overlay)."""
    xs = [clamp(landmarks[index] * width, 0.0, width) for index in range(0, len(landmarks), 2)]
    ys = [clamp(landmarks[index] * height, 0.0, height) for index in range(1, len(landmarks), 2)]
    return [min(xs), min(ys), max(xs), max(ys)]


def build_results(faces: Sequence[Any], width: float, height: float) -> list[FaceEyes]:
    """Combine each detected face's landmarks into its overall bounds plus its two eye boxes."""
    width = require_positive_finite(width, "width")
    height = require_positive_finite(height, "height")
    results: list[FaceEyes] = []
    for face in faces:
        values = [float(value) for value in face]
        boxes = decode_eye_boxes(values, width, height)
        results.append(
            {
                "face_box": face_bounds(values, width, height),
                "eyes": [
                    {"label": "left_eye", "box": boxes["left_eye"]},
                    {"label": "right_eye", "box": boxes["right_eye"]},
                ],
            }
        )
    return results


def results_json(results: Sequence[FaceEyes]) -> str:
    """Serialise the per-face eye results to JSON for the renderer."""
    return json.dumps(list(results))


def client_event_json(details: dict[str, object]) -> str:
    """Build the et-client-event JSON envelope for an eye-detection inference."""
    return WsClientEvent(
        type="et-client-event",
        capability="eye_detection",
        action="inference",
        details=details,
    ).model_dump_json()


def status_text(results: Sequence[FaceEyes]) -> str:
    """Render the browser status text used by the eye detection demo."""
    eye_count = sum(len(result["eyes"]) for result in results)
    lines = [
        "pyeye1 eye detection demo",
        f"model file: {EYE_MODEL_PATH}",
        f"faces: {len(results)}",
        f"eyes: {eye_count}",
        f"processed at: {datetime.now().strftime('%X')}",
    ]

    if results and results[0]["eyes"]:
        box = results[0]["eyes"][0]["box"]
        lines.extend(["", f"first eye: {box[0]:.1f}, {box[1]:.1f}, {box[2]:.1f}, {box[3]:.1f}"])

    return "\n".join(lines)


def event_payload(results: Sequence[FaceEyes], width: float, height: float) -> dict[str, object]:
    """Build the WebSocket client event payload for one eye-detection inference."""
    width = require_non_negative_finite(width, "width")
    height = require_non_negative_finite(height, "height")
    eye_count = sum(len(result["eyes"]) for result in results)
    return {
        "mode": "eye_detection",
        "detected_class": "eyes" if eye_count else "no_detection",
        "faces": len(results),
        "eyes": eye_count,
        "results": list(results),
        "processed_at": datetime.now().strftime("%X"),
        "model_path": EYE_MODEL_PATH,
        "source_resolution": {"width": float(width), "height": float(height)},
    }


def clamp(value: float, minimum: float, maximum: float) -> float:
    """Clamp `value` to the inclusive range [minimum, maximum]."""
    return max(minimum, min(value, maximum))


def require_positive_finite(value: float, name: str) -> float:
    """Return `value` as a float, raising if it isn't positive and finite."""
    value = float(value)
    if not math.isfinite(value) or value <= 0.0:
        raise ValueError(f"{name} must be a positive finite number")
    return value


def require_non_negative_finite(value: float, name: str) -> float:
    """Return `value` as a float, raising if it's negative or non-finite."""
    value = float(value)
    if not math.isfinite(value) or value < 0.0:
        raise ValueError(f"{name} must be a non-negative finite number")
    return value
