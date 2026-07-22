"""Eye-misalignment and rhythmic-oscillation screening heuristics over FaceLandmarker iris landmarks.

MediaPipe FaceLandmarker's refined mesh carries five iris landmarks per eye (a center plus a four-point ring)
alongside the eyelid and eye-corner contour points. Each video frame is reduced to one `GazeSample`: per eye,
the iris center is projected onto the corner-to-corner eye axis (horizontal ratio) and onto the upper-to-lower
eyelid axis (vertical ratio), both normalized so 0.5 means "centered in the eye opening". Ratios are unitless
fractions of the eye's own width/height, which keeps them stable across face sizes, camera distances, and
moderate head roll.

Two windowed screenings run over the sample history, each named for exactly what it measures (these are the
gaze patterns an eye-care professional would investigate as strabismus and nystagmus respectively, but no
diagnosis is made or implied):

- Eye misalignment indicator -- the two eyes point in sustainedly different directions, measured as the
  left-minus-right ratio difference staying offset. Both eye axes are ordered image-left to image-right, so a
  conjugate gaze shift (both eyes looking the same way) moves both ratios together and cancels in the
  difference; only true misalignment survives the mean.
- Rhythmic oscillation indicator -- the eyes shake rhythmically instead of holding steady, showing up as a
  periodic signal in the conjugate (two-eye mean) ratio series. The series is detrended with a centered
  moving mean (removing smooth pursuit and slow head drift), then rated by zero-crossing frequency and
  RMS-derived amplitude.

Samples taken mid-blink (eyelid gap under `MIN_OPENNESS` of the eye width) are dropped before analysis, since
a closing lid drags the detected iris and would fake both indicators.

These are screening heuristics over webcam landmarks -- NOT a medical diagnosis. Thresholds are conservative
defaults chosen to reject landmark jitter, not clinically calibrated values.
"""

from __future__ import annotations

import math
from collections.abc import Iterable, Sequence
from itertools import pairwise
from statistics import fmean
from typing import Any, TypedDict

# FaceLandmarker returns 478 landmarks: the 468-point face mesh plus one five-point iris cluster per eye (a
# center followed by a four-point ring), each x/y normalized to [0, 1] against the input frame.
MESH_LANDMARK_COUNT = 478

RIGHT_IRIS_CENTER = 468
RIGHT_IRIS_RING = (469, 470, 471, 472)
LEFT_IRIS_CENTER = 473
LEFT_IRIS_RING = (474, 475, 476, 477)

# Eye-corner pairs ordered image-left -> image-right on a non-mirrored frame ("right" is the subject's right
# eye, which appears on the left of the image). Keeping both axes in image order makes a conjugate gaze shift
# move both horizontal ratios the same direction, so it cancels in the misalignment left-minus-right
# difference.
RIGHT_EYE_AXIS = (33, 133)
LEFT_EYE_AXIS = (362, 263)
# Upper then lower eyelid mid landmarks; they define the vertical ratio axis and the blink guard.
RIGHT_EYE_LIDS = (159, 145)
LEFT_EYE_LIDS = (386, 374)

# Samples where either eye's lid gap is under this fraction of the eye width count as blinks and are excluded.
MIN_OPENNESS = 0.15
# Both screenings need this many valid samples (and oscillation this much time span) before rating a window.
MIN_ANALYSIS_SAMPLES = 12
MIN_ANALYSIS_SPAN_S = 1.0

# Sustained left-minus-right ratio offset (a fraction of each eye's own width/height) that flags misalignment.
MISALIGNMENT_RATIO_THRESHOLD = 0.10

# Centered moving-mean span for detrending; spans longer than one oscillation period would erase the signal.
DETREND_SPAN_S = 0.4
# Pathological rhythmic eye movement beats at roughly 2-10 Hz. The amplitude floor rejects landmark jitter,
# which crosses zero often but stays tiny, and the crossing floor rejects a one-off saccade or two within the
# window.
OSCILLATION_MIN_HZ = 2.0
OSCILLATION_MAX_HZ = 10.0
OSCILLATION_MIN_AMPLITUDE = 0.04
OSCILLATION_MIN_CROSSINGS = 6


class EyeGaze(TypedDict):
    """One eye's gaze ratios: iris position within the eye opening, plus how open the eyelids are."""

    h: float
    v: float
    openness: float


class GazeSample(TypedDict):
    """Both eyes' gaze ratios at one video timestamp (seconds since the workflow started)."""

    t: float
    left: EyeGaze
    right: EyeGaze
    valid: bool


class MisalignmentMetrics(TypedDict):
    """Windowed eye-alignment metrics; deviations are mean left-minus-right ratio differences."""

    status: str
    detected: bool
    horizontal_deviation: float | None
    vertical_deviation: float | None


class OscillationMetrics(TypedDict):
    """Windowed rhythmic-oscillation metrics for the dominant axis of the conjugate gaze signal."""

    status: str
    detected: bool
    axis: str | None
    frequency_hz: float | None
    amplitude: float | None


class WindowAnalysis(TypedDict):
    """One analysis pass over the sliding sample window: counts plus both screening results."""

    window_ms: float
    samples: int
    valid_samples: int
    misalignment: MisalignmentMetrics
    oscillation: OscillationMetrics


def landmark_px(values: Sequence[float], index: int, width: float, height: float) -> tuple[float, float]:
    """Return one normalized landmark as an (x, y) point in source pixels."""
    return (values[index * 2] * width, values[index * 2 + 1] * height)


def eye_gaze(
    values: Sequence[float],
    width: float,
    height: float,
    axis: tuple[int, int],
    lids: tuple[int, int],
    iris_center: int,
) -> EyeGaze:
    """Project one eye's iris center onto its corner and eyelid axes, yielding normalized gaze ratios."""
    corner_ax, corner_ay = landmark_px(values, axis[0], width, height)
    corner_bx, corner_by = landmark_px(values, axis[1], width, height)
    dx, dy = corner_bx - corner_ax, corner_by - corner_ay
    axis_len_sq = dx * dx + dy * dy
    if axis_len_sq <= 0.0:
        raise ValueError("eye corner landmarks are degenerate")
    iris_x, iris_y = landmark_px(values, iris_center, width, height)
    h = ((iris_x - corner_ax) * dx + (iris_y - corner_ay) * dy) / axis_len_sq

    upper_x, upper_y = landmark_px(values, lids[0], width, height)
    lower_x, lower_y = landmark_px(values, lids[1], width, height)
    vx, vy = lower_x - upper_x, lower_y - upper_y
    lid_len_sq = vx * vx + vy * vy
    # A fully closed lid collapses the vertical axis; report a centered ratio and let `openness` gate it out.
    v = 0.5 if lid_len_sq <= 0.0 else ((iris_x - upper_x) * vx + (iris_y - upper_y) * vy) / lid_len_sq
    return {"h": h, "v": v, "openness": math.sqrt(lid_len_sq / axis_len_sq)}


def gaze_sample(landmarks: Iterable[Any], width: float, height: float, t: float) -> GazeSample:
    """Build one two-eye gaze sample from a face's flat normalized FaceLandmarker landmarks."""
    values = [float(value) for value in landmarks]
    if len(values) < MESH_LANDMARK_COUNT * 2:
        raise ValueError("FaceLandmarker output did not contain the iris landmarks")
    left = eye_gaze(values, width, height, LEFT_EYE_AXIS, LEFT_EYE_LIDS, LEFT_IRIS_CENTER)
    right = eye_gaze(values, width, height, RIGHT_EYE_AXIS, RIGHT_EYE_LIDS, RIGHT_IRIS_CENTER)
    valid = left["openness"] >= MIN_OPENNESS and right["openness"] >= MIN_OPENNESS
    return {"t": float(t), "left": left, "right": right, "valid": valid}


def misalignment_metrics(samples: Sequence[GazeSample]) -> MisalignmentMetrics:
    """Rate windowed eye alignment: a sustained left-minus-right ratio offset flags the indicator."""
    valid = [sample for sample in samples if sample["valid"]]
    if len(valid) < MIN_ANALYSIS_SAMPLES:
        return {
            "status": "insufficient_data",
            "detected": False,
            "horizontal_deviation": None,
            "vertical_deviation": None,
        }
    dh = fmean(sample["left"]["h"] - sample["right"]["h"] for sample in valid)
    dv = fmean(sample["left"]["v"] - sample["right"]["v"] for sample in valid)
    return {
        "status": "ok",
        "detected": abs(dh) > MISALIGNMENT_RATIO_THRESHOLD or abs(dv) > MISALIGNMENT_RATIO_THRESHOLD,
        "horizontal_deviation": round(dh, 4),
        "vertical_deviation": round(dv, 4),
    }


def detrended(ts: Sequence[float], xs: Sequence[float]) -> list[float]:
    """Subtract a centered moving mean (span `DETREND_SPAN_S`) so smooth pursuit and slow drift drop out."""
    half_span = DETREND_SPAN_S / 2.0
    residual: list[float] = []
    # O(n^2) over a ~50-sample window is trivially cheap and tolerates the irregular timestamps blinks leave.
    for t, x in zip(ts, xs, strict=True):
        local = [value for stamp, value in zip(ts, xs, strict=True) if abs(stamp - t) <= half_span]
        residual.append(x - fmean(local))
    return residual


def series_oscillation(ts: Sequence[float], xs: Sequence[float]) -> tuple[float, float, int]:
    """Return (frequency_hz, amplitude, zero_crossings) of the detrended series via zero-crossing counting."""
    residual = detrended(ts, xs)
    duration = ts[-1] - ts[0]
    crossings = sum(1 for a, b in pairwise(residual) if a * b < 0.0)
    frequency = crossings / (2.0 * duration) if duration > 0.0 else 0.0
    rms = math.sqrt(fmean(value * value for value in residual))
    # For a pure sinusoid RMS * sqrt(2) recovers the peak amplitude; landmark jitter stays well below it.
    return frequency, rms * math.sqrt(2.0), crossings


def oscillation_metrics(samples: Sequence[GazeSample]) -> OscillationMetrics:
    """Rate windowed rhythmic movement of the conjugate (two-eye mean) gaze signal, per axis."""
    valid = [sample for sample in samples if sample["valid"]]
    if len(valid) < MIN_ANALYSIS_SAMPLES or valid[-1]["t"] - valid[0]["t"] < MIN_ANALYSIS_SPAN_S:
        return {
            "status": "insufficient_data",
            "detected": False,
            "axis": None,
            "frequency_hz": None,
            "amplitude": None,
        }

    ts = [sample["t"] for sample in valid]
    horizontal = [(sample["left"]["h"] + sample["right"]["h"]) / 2.0 for sample in valid]
    vertical = [(sample["left"]["v"] + sample["right"]["v"]) / 2.0 for sample in valid]

    # Rate each axis, then report a passing axis if any (the larger-amplitude one), else the dominant axis.
    rated: list[tuple[bool, float, float, int, str]] = []
    for axis_label, series in (("horizontal", horizontal), ("vertical", vertical)):
        frequency, amplitude, crossings = series_oscillation(ts, series)
        passes = (
            OSCILLATION_MIN_HZ <= frequency <= OSCILLATION_MAX_HZ
            and amplitude >= OSCILLATION_MIN_AMPLITUDE
            and crossings >= OSCILLATION_MIN_CROSSINGS
        )
        rated.append((passes, amplitude, frequency, crossings, axis_label))
    passes, amplitude, frequency, _crossings, axis_label = max(rated)
    return {
        "status": "ok",
        "detected": passes,
        "axis": axis_label,
        "frequency_hz": round(frequency, 2),
        "amplitude": round(amplitude, 4),
    }


def analyze_window(samples: Sequence[GazeSample]) -> WindowAnalysis:
    """Run both screenings over the current sliding window of gaze samples."""
    window_ms = (samples[-1]["t"] - samples[0]["t"]) * 1000.0 if samples else 0.0
    return {
        "window_ms": round(window_ms, 1),
        "samples": len(samples),
        "valid_samples": sum(1 for sample in samples if sample["valid"]),
        "misalignment": misalignment_metrics(samples),
        "oscillation": oscillation_metrics(samples),
    }
