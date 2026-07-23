"""Decision and event helpers for browser-side speech detection."""

from __future__ import annotations

import math
from collections.abc import Iterable
from datetime import datetime, timezone
from typing import Any, TypedDict

from et_ws.messages import WsClientEvent

SPEECH_MODEL_PATH = "/modules/et-model-speech1/speech1.onnx"
SAMPLE_RATE = 16_000
CHUNK_SIZE = 512
CONTEXT_SIZE = 64
CAPTURE_SECONDS = 30
SPEECH_THRESHOLD = 0.5
NEGATIVE_THRESHOLD = 0.35
MIN_SPEECH_MS = 250
MIN_SILENCE_MS = 100


class SpeechSummary(TypedDict):
    """A compact result derived from one microphone capture."""

    speech_detected: bool
    confidence: float
    mean_probability: float
    speech_ratio: float
    speech_duration_ms: int
    frame_count: int
    processed_at: str


def config() -> dict[str, object]:
    """Return constants used by the browser adapter."""
    return {
        "model_path": SPEECH_MODEL_PATH,
        "sample_rate": SAMPLE_RATE,
        "chunk_size": CHUNK_SIZE,
        "context_size": CONTEXT_SIZE,
        "capture_seconds": CAPTURE_SECONDS,
    }


def summarize_probabilities(values: Iterable[object]) -> SpeechSummary:
    """Apply hysteresis and minimum-duration filtering."""
    probabilities = [_probability(value) for value in values]
    if not probabilities:
        raise ValueError("Speech detection model returned no probabilities")

    chunk_ms = CHUNK_SIZE * 1000.0 / SAMPLE_RATE
    min_speech_frames = math.ceil(MIN_SPEECH_MS / chunk_ms)
    min_silence_frames = math.ceil(MIN_SILENCE_MS / chunk_ms)
    detected_frames = 0
    longest_segment = 0
    segment_frames = 0
    silence_frames = 0
    triggered = False

    for probability in probabilities:
        if probability >= SPEECH_THRESHOLD:
            if not triggered:
                triggered = True
                segment_frames = 0
            silence_frames = 0
            segment_frames += 1
            detected_frames += 1
        elif triggered:
            segment_frames += 1
            if probability < NEGATIVE_THRESHOLD:
                silence_frames += 1
                if silence_frames >= min_silence_frames:
                    longest_segment = max(longest_segment, segment_frames - silence_frames)
                    triggered = False
                    segment_frames = 0
                    silence_frames = 0
            else:
                silence_frames = 0

    if triggered:
        longest_segment = max(longest_segment, segment_frames - silence_frames)

    return {
        "speech_detected": longest_segment >= min_speech_frames,
        "confidence": max(probabilities),
        "mean_probability": sum(probabilities) / len(probabilities),
        "speech_ratio": detected_frames / len(probabilities),
        "speech_duration_ms": round(longest_segment * chunk_ms),
        "frame_count": len(probabilities),
        "processed_at": datetime.now(timezone.utc).isoformat(),
    }


def event_payload(summary: SpeechSummary, source_sample_rate: float, recorded_seconds: float) -> dict[str, object]:
    """Build the details object sent to the websocket."""
    return {
        **summary,
        "label": "speech" if summary["speech_detected"] else "no_speech",
        "source_sample_rate": round(source_sample_rate),
        "model_sample_rate": SAMPLE_RATE,
        "recorded_seconds": round(recorded_seconds, 3),
        "threshold": SPEECH_THRESHOLD,
    }


def client_event_json(details: dict[str, object]) -> str:
    """Build the typed et-client-event envelope."""
    return WsClientEvent(
        type="et-client-event",
        capability="speech_detection",
        action="inference",
        details=details,
    ).model_dump_json()


async def run(infer_capture, send_event, render_result, log, set_status) -> None:
    """Capture once through JS, classify in Python, and publish the result."""
    capture = await infer_capture()
    summary = summarize_probabilities(capture["probabilities"])
    details = event_payload(summary, capture["source_sample_rate"], capture["recorded_seconds"])
    render_result(summary["speech_detected"], summary["confidence"])
    set_status(status_text(summary))
    send_event(client_event_json(details))
    log(f"speech_detected={summary['speech_detected']} confidence={summary['confidence']:.3f}")


def status_text(summary: SpeechSummary) -> str:
    """Render a concise human-readable detection result."""
    label = "SPEECH DETECTED" if summary["speech_detected"] else "NO SPEECH DETECTED"
    return (
        f"pyspeech1: {label}\n"
        f"peak probability: {summary['confidence']:.3f}\n"
        f"mean probability: {summary['mean_probability']:.3f}\n"
        f"speech ratio: {summary['speech_ratio']:.1%}\n"
        f"longest speech segment: {summary['speech_duration_ms']} ms"
    )


def starting_status() -> str:
    """Return the status shown while microphone audio is captured."""
    return f"pyspeech1 speech detection: recording for {CAPTURE_SECONDS} seconds"


def stopped_status() -> str:
    """Return the status shown after an explicit stop."""
    return "pyspeech1 speech detection stopped."


def model_log_message() -> str:
    """Return the model-loading log message."""
    return f"loading FP16 speech detection model from {SPEECH_MODEL_PATH}"


def _probability(value: Any) -> float:
    probability = float(value)
    if not math.isfinite(probability) or not 0.0 <= probability <= 1.0:
        raise ValueError(f"invalid speech probability: {value}")
    return probability
