"""Composition helpers that reuse pyeye1 and pyspeech1 inference post-processing."""

from __future__ import annotations

import json
import time
from collections import deque
from typing import Any, TypedDict

from pyeye1.eye_detection import (
    ANALYSIS_INTERVAL_MS,
    ANALYSIS_WINDOW_MS,
    EYE_MODEL_PATH,
    PERIODIC_CAPTURE_INTERVAL_MS,
    VISION_BUNDLE_PATH,
    VISION_WASM_PATH,
    build_results,
    capture_broadcast_json,
    eye_capture_error_event_json,
    eye_region_crop,
    results_json,
    smooth_crop,
)
from pyeye1.eye_detection import (
    client_event_json as eye_event_json,
)
from pyeye1.eye_detection import (
    event_payload as eye_event_payload,
)
from pyeye1.gaze_analysis import GazeSample, WindowAnalysis, analyze_window, gaze_sample
from pyspeech1.speech_detection import (
    SPEECH_THRESHOLD,
    summarize_probabilities,
)
from pyspeech1.speech_detection import (
    client_event_json as speech_event_json,
)
from pyspeech1.speech_detection import config as speech_config
from pyspeech1.speech_detection import (
    event_payload as speech_event_payload,
)

POLL_INTERVAL_MS = 100
WS_CONNECT_TIMEOUT_MS = 10_000


async def run(platform: Any) -> None:
    """Drive the combined demo while JavaScript supplies only browser-facing operations.

    The launcher remains a JavaScript concern, but after it is pressed Python owns the ordered workflow and
    its timeout.  This mirrors pyeye1's platform pattern and guarantees media cleanup on setup, inference,
    cancellation, and speech-analysis failures.
    """
    try:
        platform.set_loading_message("Connecting to the local service...")
        await platform.connect_ws()
        connected = await wait_until_connected(platform)
        if not connected or platform.should_stop():
            return
        platform.log(f"websocket connected with agent_id={platform.agent_id()}")

        platform.set_loading_message("Loading detection models...")
        await platform.load_models()
        if platform.should_stop():
            return
        platform.show_demo()
        await platform.capture()
    finally:
        platform.cleanup()


async def wait_until_connected(platform: Any) -> bool:
    """Wait for the WebSocket connection, returning early when the demo is cancelled."""
    waited_ms = 0
    while platform.ws_state() != "connected":
        if platform.should_stop():
            return False
        if waited_ms >= WS_CONNECT_TIMEOUT_MS:
            raise RuntimeError("WebSocket connection timed out")
        await platform.sleep(POLL_INTERVAL_MS)
        waited_ms += POLL_INTERVAL_MS
    return True


class EyeCaptureResult(TypedDict):
    """Browser-ready eye overlay and websocket event."""

    results_json: str
    event_json: str | None
    face_count: int
    eye_count: int
    capture_count: int


class EyeCaptureProcessor:
    """Retain pyeye1's rolling gaze window, analysis cadence, and smoothed crop across demo frames."""

    def __init__(self) -> None:
        """Initialize an empty eye-analysis session."""
        self.reset()

    def reset(self) -> None:
        """Start a fresh eye-screening window for one combined-demo recording."""
        self.started_at = time.monotonic()
        self.last_analysis_ms = 0.0
        self.history: deque[GazeSample] = deque()
        self.analysis: WindowAnalysis | None = None
        self.crop: list[float] | None = None
        self.last_periodic_capture_ms = 0.0
        self.indicator_was_active = False
        self.captured_for_episode = False

    def process(self, capture: dict[str, Any]) -> EyeCaptureResult:
        """Process one FaceLandmarker capture with the current pyeye1 rolling-window functionality."""
        faces = capture["faces"]
        width, height = capture["width"], capture["height"]
        upload_consent = bool(capture.get("upload_consent", False))
        results = build_results(faces, width, height)
        now_s = time.monotonic() - self.started_at
        if results:
            self.crop = smooth_crop(self.crop, eye_region_crop(results[0], width, height))
        if faces:
            self.history.append(gaze_sample(faces[0], width, height, now_s))
        while self.history and (now_s - self.history[0]["t"]) * 1000.0 > ANALYSIS_WINDOW_MS:
            self.history.popleft()

        event_json: str | None = None
        capture_count = 0
        if now_s * 1000.0 - self.last_analysis_ms >= ANALYSIS_INTERVAL_MS:
            self.last_analysis_ms = now_s * 1000.0
            self.analysis = analyze_window(list(self.history))
            event_json = eye_event_json(eye_event_payload(results, self.analysis, width, height))
            indicator_active = self.analysis["misalignment"]["detected"] or self.analysis["oscillation"]["detected"]
            if indicator_active and not self.indicator_was_active:
                self.captured_for_episode = False
            if indicator_active and not self.captured_for_episode and upload_consent:
                self.captured_for_episode = True
                capture_count += 1
            self.indicator_was_active = indicator_active

        if now_s * 1000.0 - self.last_periodic_capture_ms >= PERIODIC_CAPTURE_INTERVAL_MS:
            self.last_periodic_capture_ms = now_s * 1000.0
            if upload_consent:
                capture_count += 1
        return {
            "results_json": results_json(results, self.analysis, self.crop),
            "event_json": event_json,
            "face_count": len(results),
            "eye_count": sum(len(result["eyes"]) for result in results),
            "capture_count": capture_count,
        }


_eye_processor = EyeCaptureProcessor()


class SpeechCaptureResult(TypedDict):
    """Browser-ready speech decision and websocket event."""

    speech_detected: bool
    confidence: float
    speech_duration_ms: int
    event_json: str


def config() -> dict[str, Any]:
    """Merge the existing eye and speech module browser configuration."""
    speech = speech_config()
    speech["threshold"] = SPEECH_THRESHOLD
    return {
        "eye": {
            "model_path": EYE_MODEL_PATH,
            "bundle_path": VISION_BUNDLE_PATH,
            "wasm_path": VISION_WASM_PATH,
        },
        "speech": speech,
    }


def process_eye_capture(capture_json: str) -> EyeCaptureResult:
    """Process one MediaPipe capture through pyeye1's current rolling gaze-screening pipeline."""
    return _eye_processor.process(json.loads(capture_json))


def reset_eye_capture() -> None:
    """Reset rolling pyeye1 state before a new combined-demo recording."""
    _eye_processor.reset()


def eye_capture_error_json(error: str) -> str:
    """Reuse pyeye1's server-visible event for a failed consented image upload."""
    return eye_capture_error_event_json(error)


def eye_capture_stored_json(agent_id: str, filename: str) -> str:
    """Reuse pyeye1's broadcast announcing a stored eye capture, so pic-viewer displays pydemo1 uploads too."""
    return capture_broadcast_json(agent_id, filename)


def process_speech_capture(
    probabilities: list[float], source_sample_rate: float, recorded_seconds: float
) -> SpeechCaptureResult:
    """Classify one probability sequence through pyspeech1."""
    summary = summarize_probabilities(probabilities)
    details = speech_event_payload(summary, source_sample_rate, recorded_seconds)
    return {
        "speech_detected": summary["speech_detected"],
        "confidence": summary["confidence"],
        "speech_duration_ms": summary["speech_duration_ms"],
        "event_json": speech_event_json(details),
    }
