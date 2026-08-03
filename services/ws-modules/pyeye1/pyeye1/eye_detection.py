"""Eye-box and eye-movement post-processing for the pyeye1 MediaPipe FaceLandmarker workflow.

This module's `run()` drives the whole workflow: WebSocket connection, camera acquisition, loading Google's
MediaPipe FaceLandmarker (a maintained, offline `.task` bundle that internally runs a face detector then a
mesh model), the sampling loop, and teardown. The browser shim contributes only a flat "platform" object of
primitives -- each a single browser operation (getUserMedia, one landmarker inference, one canvas draw) with
no sequencing, polling, or timeout logic of its own. Each frame's landmarks become per-eye bounding boxes
plus iris circles for the overlay, and a rolling window of per-frame gaze samples feeds the eye-misalignment
and rhythmic-oscillation screening heuristics in `gaze_analysis`.
"""

from __future__ import annotations

import json
import math
import time
from collections import deque
from collections.abc import Iterable, Sequence
from datetime import datetime, timezone
from statistics import fmean
from typing import Any, TypedDict

from et_ws.messages import WsBroadcastMessage, WsClientEvent

from .gaze_analysis import (
    LEFT_IRIS_CENTER,
    LEFT_IRIS_RING,
    MESH_LANDMARK_COUNT,
    RIGHT_IRIS_CENTER,
    RIGHT_IRIS_RING,
    GazeSample,
    WindowAnalysis,
    analyze_window,
    gaze_sample,
)

# Served by the MediaPipe tasks-vision runtime module and the model module (see config()).
EYE_MODEL_PATH = "/modules/et-model-eye1/face_landmarker.task"
VISION_BUNDLE_PATH = "/modules/@mediapipe/tasks-vision/vision_bundle.mjs"
VISION_WASM_PATH = "/modules/@mediapipe/tasks-vision/wasm"

# The eye-box decode needs only the face-mesh contour indices (all below this count); the iris circles and the
# gaze screening additionally need the refined iris landmarks (see `gaze_analysis.MESH_LANDMARK_COUNT`).
CONTOUR_LANDMARK_COUNT = 468
# Eye-contour landmark indices in the MediaPipe mesh (subject's perspective, so "left" appears on the right of
# a non-mirrored frame). A tight per-eye box is the min/max over each cluster.
RIGHT_EYE_INDICES = (33, 7, 163, 144, 145, 153, 154, 155, 133, 173, 157, 158, 159, 160, 161, 246)
LEFT_EYE_INDICES = (263, 249, 390, 373, 374, 380, 381, 382, 362, 398, 384, 385, 386, 387, 388, 466)

# Frames are sampled continuously (the cadence must outpace the 2-10 Hz oscillations `gaze_analysis` screens
# for), while status updates and WebSocket events go out at the slower analysis cadence.
SAMPLE_INTERVAL_MS = 50
MIN_SLEEP_MS = 10
ANALYSIS_INTERVAL_MS = 1000
ANALYSIS_WINDOW_MS = 2500
MAX_SAMPLES = 600
MAX_RUNTIME_MS = 30_000
# Independent of the detection-triggered capture below: a fixed-cadence "heartbeat" capture so at least one
# image gets stored periodically even across a long session where the screening indicators never fire.
PERIODIC_CAPTURE_INTERVAL_MS = 5_000

# Setup polling. Python owns all sequencing and timeouts; the JS primitives never loop or wait on their own.
POLL_INTERVAL_MS = 100
WS_CONNECT_TIMEOUT_MS = 10_000
VIDEO_READY_TIMEOUT_MS = 5_000

# The overlay canvas shows only a face band around the eyes: the face's width (plus a small margin) by a
# vertical band centered on the eye line. The band's half-height is a fraction of the face height (floored at
# the eye cluster's own height), and the box is exponentially smoothed so per-frame landmark jitter and blinks
# don't make the cropped view judder.
CROP_HORIZONTAL_MARGIN = 0.04
CROP_VERTICAL_EXTENT = 0.22
CROP_SMOOTHING = 0.35


Box = list[float]


class Eye(TypedDict):
    """One detected eye: its label and bounding box in source-image pixels."""

    label: str
    box: Box


class Iris(TypedDict):
    """One detected iris: its eye label, center point, and radius in source-image pixels."""

    label: str
    center: list[float]
    radius: float


class FaceEyes(TypedDict):
    """One detected face: its overall landmark bounds plus the eye boxes and iris circles within it."""

    face_box: Box
    eyes: list[Eye]
    irises: list[Iris]


async def run(platform) -> None:
    """Drive the whole eye movement screening workflow, using JS only for the browser primitives.

    `platform` supplies the primitives Python cannot implement itself, each a single browser operation with
    no sequencing, polling, or timeout logic. Sync members: `ws_state()`, `agent_id()`, `send_event(json)`,
    `video_size() -> [w, h]`, `render(json)`, `log(str)`, `set_status(str)`, `should_stop()`, `cleanup()`,
    `upload_consent() -> bool` (the page's data-upload checkbox). Async members: `connect_ws()`,
    `start_camera()`, `play_video()`, `sleep(ms)`, `load_landmarker(model_path, bundle_path, wasm_path)`,
    `save_eye_capture()` (encodes the current overlay canvas as a PNG, uploads it to the connected agent's
    storage bucket, and returns the stored filename), and
    `infer()`, which returns one FaceLandmarker pass as the JSON string
    `{"faces": [[x0, y0, x1, y1, ...], ...], "width": W, "height": H}` where each face is the flat list of
    normalized landmark coordinates.
    """
    platform.set_status(starting_status())
    platform.log(model_log_message())
    try:
        await connect_websocket(platform)
        await acquire_camera(platform)
        await platform.load_landmarker(EYE_MODEL_PATH, VISION_BUNDLE_PATH, VISION_WASM_PATH)
        await sample_loop(platform)
    finally:
        platform.cleanup()


async def connect_websocket(platform) -> None:
    """Open the WebSocket client and wait until the server acknowledges the agent connection."""
    await platform.connect_ws()
    await wait_until(
        platform,
        lambda: platform.ws_state() == "connected",
        WS_CONNECT_TIMEOUT_MS,
        "Timed out waiting for websocket connection",
    )
    platform.log(f"websocket connected with agent_id={platform.agent_id()}")


async def acquire_camera(platform) -> None:
    """Start the camera stream, wait for the video element to report its frame size, and begin playback."""
    await platform.start_camera()

    def video_ready() -> bool:
        size = platform.video_size()
        return size[0] > 0 and size[1] > 0

    await wait_until(platform, video_ready, VIDEO_READY_TIMEOUT_MS, "Video stream metadata did not load")
    await platform.play_video()


async def wait_until(platform, predicate, timeout_ms: float, failure: str) -> None:
    """Poll `predicate` every `POLL_INTERVAL_MS` until it holds, raising `RuntimeError(failure)` on timeout."""
    waited_ms = 0.0
    while not predicate():
        if waited_ms >= timeout_ms:
            raise RuntimeError(failure)
        await platform.sleep(POLL_INTERVAL_MS)
        waited_ms += POLL_INTERVAL_MS


async def attempt_eye_capture(platform) -> None:
    """Try one `save_eye_capture()`; broadcast a success to other agents, report a failure without raising.

    Shared by the two independent capture triggers in `sample_loop` (a detection's rising edge, and the
    fixed-cadence periodic heartbeat) so both behave identically: each stored capture is announced to every
    other connected agent (the pic-viewer module running on another device listens for these announcements
    and displays the file), and a failed upload is logged locally plus reported as a server-visible event,
    never allowed to abort the sample loop or be misreported as an inference error.
    """
    try:
        filename = await platform.save_eye_capture()
    # Broad by contract: the capture crosses into JS (canvas encode, fetch upload), which surfaces arbitrary
    # exception types through Pyodide, and this function's whole purpose is to report any of them without raising.
    except Exception as exc:  # noqa: BLE001
        platform.log(f"eye capture failed: {exc}")
        platform.send_event(eye_capture_error_event_json(str(exc)))
        return
    platform.send_event(capture_broadcast_json(str(platform.agent_id()), str(filename)))


async def sample_loop(platform) -> None:
    """Sample frames continuously; re-analyze the gaze window (status + event) per `ANALYSIS_INTERVAL_MS`."""
    sample_count = 0
    started_at = time.monotonic()
    results: list[FaceEyes] = []
    history: deque[GazeSample] = deque()
    analysis: WindowAnalysis | None = None
    crop: Box | None = None
    last_analysis_ms = 0.0
    last_periodic_capture_ms = 0.0
    indicator_was_active = False
    captured_for_episode = False

    while not platform.should_stop():
        loop_started = time.monotonic()
        if sample_count >= MAX_SAMPLES or (loop_started - started_at) * 1000.0 >= MAX_RUNTIME_MS:
            break

        try:
            capture = json.loads(await platform.infer())
            now_s = time.monotonic() - started_at
            width, height = capture["width"], capture["height"]
            results = build_results(capture["faces"], width, height)
            sample_count += 1

            # The gaze window and the cropped view track the first (and, with numFaces=1, only) face; while
            # no face is visible the last smoothed crop is kept rather than snapping back to the full frame.
            if results:
                crop = smooth_crop(crop, eye_region_crop(results[0], width, height))
            if capture["faces"]:
                history.append(gaze_sample(capture["faces"][0], width, height, now_s))
            while history and (now_s - history[0]["t"]) * 1000.0 > ANALYSIS_WINDOW_MS:
                history.popleft()

            is_analysis_tick = now_s * 1000.0 - last_analysis_ms >= ANALYSIS_INTERVAL_MS
            if is_analysis_tick:
                last_analysis_ms = now_s * 1000.0
                analysis = analyze_window(list(history))
                platform.set_status(status_text(results, analysis))
                platform.send_event(client_event_json(event_payload(results, analysis, width, height)))

                # Save one eye capture per detection -- "detection" means a screening indicator (eye
                # misalignment or rhythmic oscillation) newly firing, not merely a face/eyes being visible.
                # Edge-triggered on the indicator's own rising edge (not-detected -> detected), tracked
                # independently of consent: a new episode resets `captured_for_episode` regardless of
                # whether consent is granted yet, so if consent arrives partway through an already-active
                # episode, that episode still gets its one capture rather than the edge having been silently
                # consumed earlier while consent was still off.
                indicator_active = analysis["misalignment"]["detected"] or analysis["oscillation"]["detected"]
                if indicator_active and not indicator_was_active:
                    captured_for_episode = False
                if indicator_active and not captured_for_episode and platform.upload_consent():
                    captured_for_episode = True
                    await attempt_eye_capture(platform)
                indicator_was_active = indicator_active

            # Independent, fixed-cadence capture on top of the detection-triggered one above: fires every
            # PERIODIC_CAPTURE_INTERVAL_MS regardless of whether a screening indicator has ever activated, so
            # a long quiet session still gets a periodic image, not only ever the first detection. The
            # interval tracks wall-clock time unconditionally (like `last_analysis_ms` above) so it stays on
            # schedule through stretches with no consent; only the capture attempt itself is gated on consent.
            is_periodic_capture_tick = now_s * 1000.0 - last_periodic_capture_ms >= PERIODIC_CAPTURE_INTERVAL_MS
            if is_periodic_capture_tick:
                last_periodic_capture_ms = now_s * 1000.0
                if platform.upload_consent():
                    await attempt_eye_capture(platform)
            platform.render(results_json(results, analysis, crop))
        # Broad by design: MediaPipe/wasm inference and the JS canvas calls raise arbitrary types, and a
        # long-running sample loop must surface the error and keep sampling rather than die on one bad frame.
        except Exception as exc:  # noqa: BLE001
            message = f"pyeye1 eye movement screening: inference error\n{exc}"
            platform.set_status(message)
            platform.log(f"inference error: {exc}")

        spent_ms = (time.monotonic() - loop_started) * 1000.0
        await platform.sleep(max(SAMPLE_INTERVAL_MS - spent_ms, MIN_SLEEP_MS))

    if sample_count >= MAX_SAMPLES:
        platform.log(f"workflow finished automatically after {MAX_SAMPLES} samples")
    elif (time.monotonic() - started_at) * 1000.0 >= MAX_RUNTIME_MS:
        platform.log("workflow finished automatically after 30 seconds")
    platform.set_status(stopped_status())


def starting_status() -> str:
    """Return the status line shown while the workflow starts up."""
    return "pyeye1 eye movement screening: starting"


def stopped_status() -> str:
    """Return the status line shown once the workflow has stopped."""
    return "pyeye1 eye movement screening demo stopped."


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


def iris_circle(
    values: Sequence[float], label: str, center_index: int, ring: Sequence[int], width: float, height: float
) -> Iris:
    """Map one iris cluster (center + four-point ring) to a labelled circle in source pixels."""
    center_x = clamp(values[center_index * 2] * width, 0.0, width)
    center_y = clamp(values[center_index * 2 + 1] * height, 0.0, height)
    distances = [
        math.hypot(values[index * 2] * width - center_x, values[index * 2 + 1] * height - center_y) for index in ring
    ]
    return {"label": label, "center": [center_x, center_y], "radius": fmean(distances)}


def decode_irises(values: Sequence[float], width: float, height: float) -> list[Iris]:
    """Decode both iris circles from one face's normalized FaceLandmarker landmarks."""
    if len(values) < MESH_LANDMARK_COUNT * 2:
        raise ValueError("FaceLandmarker output did not contain the iris landmarks")
    return [
        iris_circle(values, "left_eye", LEFT_IRIS_CENTER, LEFT_IRIS_RING, width, height),
        iris_circle(values, "right_eye", RIGHT_IRIS_CENTER, RIGHT_IRIS_RING, width, height),
    ]


def face_bounds(landmarks: Sequence[float], width: float, height: float) -> Box:
    """Return the bounding box of all of a face's landmarks, in source pixels (context for the eye overlay)."""
    xs = [clamp(landmarks[index] * width, 0.0, width) for index in range(0, len(landmarks), 2)]
    ys = [clamp(landmarks[index] * height, 0.0, height) for index in range(1, len(landmarks), 2)]
    return [min(xs), min(ys), max(xs), max(ys)]


def build_results(faces: Sequence[Any], width: float, height: float) -> list[FaceEyes]:
    """Combine each face's landmarks into its overall bounds, its two eye boxes, and its iris circles."""
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
                "irises": decode_irises(values, width, height),
            }
        )
    return results


def eye_region_crop(result: FaceEyes, width: float, height: float) -> Box:
    """Return the source-pixel crop showing only the face band around the eyes, clamped to the frame."""
    face = result["face_box"]
    eye_boxes = [eye["box"] for eye in result["eyes"]]
    eye_top = min(box[1] for box in eye_boxes)
    eye_bottom = max(box[3] for box in eye_boxes)
    band_center = (eye_top + eye_bottom) / 2.0
    half_band = max(CROP_VERTICAL_EXTENT * (face[3] - face[1]), eye_bottom - eye_top)
    margin_x = CROP_HORIZONTAL_MARGIN * (face[2] - face[0])
    return [
        clamp(face[0] - margin_x, 0.0, width),
        clamp(band_center - half_band, 0.0, height),
        clamp(face[2] + margin_x, 0.0, width),
        clamp(band_center + half_band, 0.0, height),
    ]


def smooth_crop(previous: Box | None, target: Box) -> Box:
    """Blend the new crop target into the previous one so the cropped view pans smoothly, not frame-jumpy."""
    if previous is None:
        return target
    return [old + CROP_SMOOTHING * (new - old) for old, new in zip(previous, target, strict=True)]


def results_json(results: Sequence[FaceEyes], analysis: WindowAnalysis | None, crop: Box | None = None) -> str:
    """Serialise the per-face results, the latest window analysis, and the view crop for the renderer."""
    return json.dumps({"faces": list(results), "analysis": analysis, "crop": crop})


def client_event_json(details: dict[str, object]) -> str:
    """Build the et-client-event JSON envelope for an eye-screening analysis pass."""
    return WsClientEvent(
        type="et-client-event",
        capability="eye_detection",
        action="inference",
        details=details,
    ).model_dump_json()


def capture_broadcast_json(agent_id: str, filename: str) -> str:
    """Build the et-broadcast-message JSON announcing a freshly stored eye capture to every other agent.

    The server relays this to each other connected agent inside an `et-agent-message` envelope. The payload's
    `kind` is the discriminator the pic-viewer module matches on, and `url` is the same-origin storage path
    it fetches and displays.
    """
    return WsBroadcastMessage(
        type="et-broadcast-message",
        message={
            "kind": "pyeye1_capture_stored",
            "agent_id": agent_id,
            "filename": filename,
            "url": f"/storage/{agent_id}/{filename}",
        },
    ).model_dump_json()


def eye_capture_error_event_json(error: str) -> str:
    """Build the et-client-event JSON envelope for a failed eye-capture upload.

    `platform.log(...)` alone only reaches the browser's own on-page log, invisible to anyone watching the
    server tty; this event puts the failure where it can actually be seen server-side, same as a successful
    capture already is (via the storage service's own "stored image" log line).
    """
    return WsClientEvent(
        type="et-client-event",
        capability="pyeye1",
        action="eye_capture_failed",
        details={"error": error},
    ).model_dump_json()


def status_text(results: Sequence[FaceEyes], analysis: WindowAnalysis | None) -> str:
    """Render the browser status text used by the eye movement screening demo."""
    eye_count = sum(len(result["eyes"]) for result in results)
    lines = [
        "pyeye1 eye movement screening demo",
        f"model file: {EYE_MODEL_PATH}",
        f"faces: {len(results)}",
        f"eyes: {eye_count}",
        *analysis_lines(analysis),
        "screening heuristics only -- not a medical diagnosis",
        f"processed at: {datetime.now(timezone.utc).strftime('%X')}",
    ]
    return "\n".join(lines)


def analysis_lines(analysis: WindowAnalysis | None) -> list[str]:
    """Summarise one window analysis as status lines (or a warming-up placeholder before the first window)."""
    if analysis is None:
        return ["analysis: warming up"]
    seconds = analysis["window_ms"] / 1000.0
    lines = [f"window: {analysis['valid_samples']}/{analysis['samples']} valid samples over {seconds:.1f}s"]

    misalignment = analysis["misalignment"]
    if misalignment["status"] == "ok":
        verdict = "DETECTED" if misalignment["detected"] else "none"
        dh = misalignment["horizontal_deviation"] or 0.0
        dv = misalignment["vertical_deviation"] or 0.0
        lines.append(f"alignment dh={dh:+.3f} dv={dv:+.3f} -> eye misalignment: {verdict}")
    else:
        lines.append("alignment: insufficient data")

    oscillation = analysis["oscillation"]
    if oscillation["status"] == "ok":
        verdict = "DETECTED" if oscillation["detected"] else "none"
        frequency = oscillation["frequency_hz"] or 0.0
        amplitude = oscillation["amplitude"] or 0.0
        axis = oscillation["axis"]
        lines.append(f"{frequency:.1f} Hz amp {amplitude:.3f} ({axis}) -> rhythmic oscillation: {verdict}")
    else:
        lines.append("oscillation: insufficient data")
    return lines


def event_payload(
    results: Sequence[FaceEyes], analysis: WindowAnalysis | None, width: float, height: float
) -> dict[str, object]:
    """Build the WebSocket client event payload for one eye-screening analysis pass."""
    width = require_non_negative_finite(width, "width")
    height = require_non_negative_finite(height, "height")
    eye_count = sum(len(result["eyes"]) for result in results)
    return {
        "mode": "eye_detection",
        "detected_class": detected_class(eye_count, analysis),
        "faces": len(results),
        "eyes": eye_count,
        "results": list(results),
        "analysis": analysis,
        "processed_at": datetime.now(timezone.utc).strftime("%X"),
        "model_path": EYE_MODEL_PATH,
        "source_resolution": {"width": float(width), "height": float(height)},
    }


def detected_class(eye_count: int, analysis: WindowAnalysis | None) -> str:
    """Pick the event's headline class: a flagged screening indicator wins over plain eye presence."""
    if analysis is not None and analysis["oscillation"]["detected"]:
        return "rhythmic_oscillation_indicator"
    if analysis is not None and analysis["misalignment"]["detected"]:
        return "eye_misalignment_indicator"
    return "eyes" if eye_count else "no_detection"


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
