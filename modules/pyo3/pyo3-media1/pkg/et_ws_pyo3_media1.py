# noqa: INP001 -- et-ws-pyo3-runner compiles this one file on its own; it is not a package
"""Capture the camera and microphone on Hiwonder's MentorPi robots, keeping the latest of each in the agent's storage.

`MEDIA_CAMERA` names the camera: its index, `0` for the first, or anything OpenCV opens -- a device such as
`/dev/video0`, a stream's URL, or a video or image file. `MEDIA_MICROPHONE` names the microphone: a sounddevice input,
by index or by part of its name, or a WAV file to replay in its place. One left unset is not captured.

`init` starts a persistent worker thread for each. Once `on_connect` has given the agent its bucket, the camera's
stores a frame as `camera-frame.jpg` at most once a second, and the microphone's stores each second it records as
`microphone-clip.wav` -- 16 kHz mono 16-bit from a device, and as recorded from a file -- until its source ends or the
runner shuts down.
"""

from __future__ import annotations

__all__ = ["init", "on_connect", "on_shutdown"]

import io
import logging
import math
import os
import threading
import time
import wave
from collections.abc import Iterator
from typing import Any

from edge_toolkit.worker import persistent_workers

_logger = logging.getLogger(__name__)

CAMERA_VARIABLE = "MEDIA_CAMERA"
MICROPHONE_VARIABLE = "MEDIA_MICROPHONE"
FRAME_KEY = "camera-frame.jpg"
CLIP_KEY = "microphone-clip.wav"
SAMPLE_RATE = 16_000

_FRAME_INTERVAL_S = 1.0
_JOIN_TIMEOUT_S = 3.0


def _source(variable: str) -> str:
    """Return the source `variable` names, logging that it is not captured when it names none."""
    source = os.environ.get(variable, "")
    if not source:
        _logger.info("%s names nothing, so it is not captured", variable)
    return source


def _watch_camera(storage: Any, stopping: threading.Event) -> None:
    """Store a frame from the camera at most once a second, until it ends or the runner shuts down."""
    source = _source(CAMERA_VARIABLE)
    if not source:
        return
    import cv2  # noqa: PLC0415 -- only a module given a camera needs OpenCV

    capture = cv2.VideoCapture(int(source) if source.isdigit() else source)
    if not capture.isOpened():
        _logger.error("could not open the camera %s", source)
        return
    stored_at = -math.inf
    stored = 0
    try:
        while not stopping.is_set():
            captured, frame = capture.read()
            if not captured:
                break
            now = time.monotonic()
            if now - stored_at >= _FRAME_INTERVAL_S:
                encoded, jpeg = cv2.imencode(".jpg", frame)
                if encoded:
                    storage.put(FRAME_KEY, jpeg.tobytes())
                    stored_at = now
                    stored += 1
    finally:
        capture.release()
    _logger.info("stored %d camera frames from %s", stored, source)


def _clips(source: str, stopping: threading.Event) -> Iterator[tuple[int, int, int, bytes]]:
    """Yield each second of `source` as its sample rate, channel count, bytes a sample and frames."""
    if source.lower().endswith(".wav"):
        with wave.open(source, "rb") as recording:
            rate = recording.getframerate()
            while frames := recording.readframes(rate):
                yield rate, recording.getnchannels(), recording.getsampwidth(), frames
        return
    import sounddevice  # noqa: PLC0415 -- only a module given a microphone device needs it

    device = int(source) if source.isdigit() else source
    with sounddevice.RawInputStream(samplerate=SAMPLE_RATE, channels=1, dtype="int16", device=device) as stream:
        while not stopping.is_set():
            frames, _overflowed = stream.read(SAMPLE_RATE)
            yield SAMPLE_RATE, 1, 2, bytes(frames)


def clip_wav(rate: int, channels: int, width: int, frames: bytes) -> bytes:
    """Render a clip as the WAV file stored."""
    buffer = io.BytesIO()
    with wave.open(buffer, "wb") as clip:
        clip.setnchannels(channels)
        clip.setsampwidth(width)
        clip.setframerate(rate)
        clip.writeframes(frames)
    return buffer.getvalue()


def _record_microphone(storage: Any, stopping: threading.Event) -> None:
    """Store each second the microphone records, until it ends or the runner shuts down."""
    source = _source(MICROPHONE_VARIABLE)
    if not source:
        return
    stored = 0
    for rate, channels, width, frames in _clips(source, stopping):
        if stopping.is_set():
            break
        storage.put(CLIP_KEY, clip_wav(rate, channels, width, frames))
        stored += 1
    _logger.info("stored %d microphone clips from %s", stored, source)


init, on_connect, on_shutdown = persistent_workers(
    {"camera": _watch_camera, "microphone": _record_microphone}, join_timeout=_JOIN_TIMEOUT_S
)
