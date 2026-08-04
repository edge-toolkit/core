import json
import unittest
from unittest.mock import patch

from pyeye1 import eye_detection
from pyeye1.eye_detection import (
    EYE_MODEL_PATH,
    VISION_BUNDLE_PATH,
    VISION_WASM_PATH,
    run,
    starting_status,
    stopped_status,
)
from pyeye1.gaze_analysis import (
    LEFT_EYE_AXIS,
    LEFT_EYE_LIDS,
    LEFT_IRIS_CENTER,
    MESH_LANDMARK_COUNT,
    RIGHT_EYE_AXIS,
    RIGHT_EYE_LIDS,
    RIGHT_IRIS_CENTER,
)


def fake_analysis(*, misalignment: bool = False, oscillation: bool = False) -> dict:
    """Build a minimal WindowAnalysis-shaped dict with a controlled misalignment/oscillation verdict.

    Used to drive the capture-triggering logic directly (rising edge of a screening indicator) without
    needing real multi-second landmark sequences to organically produce a detection through the actual
    gaze-analysis math, which is already covered by test_gaze_analysis.py.
    """
    return {
        "window_ms": 1000.0,
        "samples": 20,
        "valid_samples": 20,
        "misalignment": {
            "status": "ok",
            "detected": misalignment,
            "horizontal_deviation": 0.2 if misalignment else 0.0,
            "vertical_deviation": 0.0,
        },
        "oscillation": {
            "status": "ok",
            "detected": oscillation,
            "axis": "horizontal" if oscillation else None,
            "frequency_hz": 4.0 if oscillation else None,
            "amplitude": 0.1 if oscillation else None,
        },
    }


def synthetic_face() -> list[float]:
    """Build one flat 478*2 landmark list with open eyes and centered irises."""
    values = [0.0] * (MESH_LANDMARK_COUNT * 2)
    points = {
        RIGHT_EYE_AXIS[0]: (0.30, 0.50),
        RIGHT_EYE_AXIS[1]: (0.40, 0.50),
        RIGHT_EYE_LIDS[0]: (0.35, 0.48),
        RIGHT_EYE_LIDS[1]: (0.35, 0.52),
        RIGHT_IRIS_CENTER: (0.35, 0.50),
        LEFT_EYE_AXIS[0]: (0.60, 0.50),
        LEFT_EYE_AXIS[1]: (0.70, 0.50),
        LEFT_EYE_LIDS[0]: (0.65, 0.48),
        LEFT_EYE_LIDS[1]: (0.65, 0.52),
        LEFT_IRIS_CENTER: (0.65, 0.50),
    }
    for index, (x, y) in points.items():
        values[index * 2] = x
        values[index * 2 + 1] = y
    return values


class FakePlatform:
    """Scripted double for the browser primitives the Python workflow drives."""

    def __init__(self, stop_after: int = 5) -> None:
        self.stop_after = stop_after
        self.infer_calls = 0
        self.events: list[str] = []
        self.rendered: list[str] = []
        self.statuses: list[str] = []
        self.logs: list[str] = []
        self.landmarker_args: tuple[str, str, str] | None = None
        self.played = False
        self.cleaned = False
        self.consent = False
        self.capture_calls = 0

    async def connect_ws(self) -> None:
        pass

    def ws_state(self) -> str:
        return "connected"

    def agent_id(self) -> str:
        return "fake-agent"

    def send_event(self, message: str) -> None:
        self.events.append(message)

    async def start_camera(self) -> None:
        pass

    def video_size(self) -> list[int]:
        return [640, 480]

    async def play_video(self) -> None:
        self.played = True

    async def load_landmarker(self, model_path: str, bundle_path: str, wasm_path: str) -> None:
        self.landmarker_args = (model_path, bundle_path, wasm_path)

    async def infer(self) -> str:
        self.infer_calls += 1
        return json.dumps({"faces": [synthetic_face()], "width": 640, "height": 480})

    def render(self, results: str) -> None:
        self.rendered.append(results)

    async def sleep(self, _ms: float) -> None:
        pass

    def log(self, message: str) -> None:
        self.logs.append(message)

    def set_status(self, status: str) -> None:
        self.statuses.append(status)

    def should_stop(self) -> bool:
        return self.infer_calls >= self.stop_after

    def cleanup(self) -> None:
        self.cleaned = True

    def upload_consent(self) -> bool:
        return self.consent

    async def save_eye_capture(self) -> None:
        self.capture_calls += 1


class NeverConnectingPlatform(FakePlatform):
    def ws_state(self) -> str:
        return "connecting"


class CameraDeniedPlatform(FakePlatform):
    async def start_camera(self) -> None:
        raise RuntimeError("Permission denied")


class MidSessionConsentPlatform(FakePlatform):
    """Consent flips on only once a few samples have already run, simulating an opt-in mid-session."""

    def upload_consent(self) -> bool:
        return self.infer_calls >= 3


class FailingCapturePlatform(FakePlatform):
    async def save_eye_capture(self) -> None:
        self.capture_calls += 1
        raise RuntimeError("upload failed")


class FailingInferencePlatform(FakePlatform):
    """Inference itself raises, exercising `sample_loop`'s broad handler rather than the capture one."""

    async def infer(self) -> str:
        self.infer_calls += 1
        raise RuntimeError("inference exploded")


class RunWorkflowTests(unittest.IsolatedAsyncioTestCase):
    async def test_happy_path_drives_the_full_workflow(self) -> None:
        platform = FakePlatform(stop_after=5)
        # Analyze every loop iteration so the event path is exercised without waiting out real time.
        with patch.object(eye_detection, "ANALYSIS_INTERVAL_MS", 0.0):
            await run(platform)

        self.assertEqual(platform.landmarker_args, (EYE_MODEL_PATH, VISION_BUNDLE_PATH, VISION_WASM_PATH))
        self.assertTrue(platform.played)
        self.assertEqual(platform.infer_calls, 5)
        self.assertEqual(len(platform.rendered), 5)
        self.assertEqual(platform.statuses[0], starting_status())
        self.assertEqual(platform.statuses[-1], stopped_status())
        self.assertTrue(any("websocket connected with agent_id=fake-agent" in line for line in platform.logs))
        self.assertTrue(platform.cleaned)

        event = json.loads(platform.events[0])
        self.assertEqual(event["capability"], "eye_detection")
        self.assertEqual(event["details"]["eyes"], 2)
        self.assertIn("analysis", event["details"])

        rendered = json.loads(platform.rendered[0])
        self.assertEqual(len(rendered["faces"]), 1)
        self.assertEqual(len(rendered["faces"][0]["irises"]), 2)
        crop = rendered["crop"]
        self.assertEqual(len(crop), 4)
        self.assertLess(crop[0], crop[2])
        self.assertLess(crop[1], crop[3])
        self.assertGreaterEqual(crop[0], 0.0)
        self.assertLessEqual(crop[2], 640.0)
        self.assertLessEqual(crop[3], 480.0)

    async def test_websocket_timeout_raises_and_still_cleans_up(self) -> None:
        platform = NeverConnectingPlatform()
        with self.assertRaisesRegex(RuntimeError, "websocket connection"):
            await run(platform)
        self.assertTrue(platform.cleaned)
        self.assertEqual(platform.infer_calls, 0)

    async def test_camera_failure_propagates_and_still_cleans_up(self) -> None:
        platform = CameraDeniedPlatform()
        with self.assertRaisesRegex(RuntimeError, "Permission denied"):
            await run(platform)
        self.assertTrue(platform.cleaned)
        self.assertEqual(platform.infer_calls, 0)

    async def test_no_capture_while_eyes_are_visible_but_no_indicator_has_fired(self) -> None:
        # Regression guard: a face/eyes being visible must never be enough on its own -- only an actual
        # screening indicator (misalignment or oscillation) firing counts as a "detection".
        platform = FakePlatform(stop_after=5)
        platform.consent = True
        with (
            patch.object(eye_detection, "ANALYSIS_INTERVAL_MS", 0.0),
            patch.object(eye_detection, "analyze_window", return_value=fake_analysis()),
        ):
            await run(platform)
        self.assertEqual(platform.capture_calls, 0)

    async def test_eye_capture_fires_once_on_the_indicators_rising_edge(self) -> None:
        platform = FakePlatform(stop_after=5)
        platform.consent = True
        # not-detected, detected (rising edge -> capture #1), still detected (no repeat), cleared,
        # detected again (a new rising edge -> capture #2).
        analyses = [
            fake_analysis(),
            fake_analysis(misalignment=True),
            fake_analysis(misalignment=True),
            fake_analysis(),
            fake_analysis(oscillation=True),
        ]
        with (
            patch.object(eye_detection, "ANALYSIS_INTERVAL_MS", 0.0),
            patch.object(eye_detection, "analyze_window", side_effect=analyses),
        ):
            await run(platform)
        self.assertEqual(platform.capture_calls, 2)

    async def test_eye_capture_skipped_entirely_without_consent(self) -> None:
        platform = FakePlatform(stop_after=5)
        with (
            patch.object(eye_detection, "ANALYSIS_INTERVAL_MS", 0.0),
            patch.object(eye_detection, "analyze_window", return_value=fake_analysis(misalignment=True)),
        ):
            await run(platform)
        self.assertEqual(platform.capture_calls, 0)
        self.assertFalse(any("eye capture" in line for line in platform.logs))

    async def test_eye_capture_fires_on_first_rising_edge_after_consent_granted_mid_session(self) -> None:
        platform = MidSessionConsentPlatform(stop_after=5)
        # Indicator is active from the very first sample; consent only turns on at infer_calls >= 3, so the
        # capture fires on sample 3 (the first tick where both are true) and not again for samples 4-5, since
        # the indicator never drops back down to re-trigger a new rising edge.
        with (
            patch.object(eye_detection, "ANALYSIS_INTERVAL_MS", 0.0),
            patch.object(eye_detection, "analyze_window", return_value=fake_analysis(misalignment=True)),
        ):
            await run(platform)
        self.assertEqual(platform.capture_calls, 1)

    async def test_inference_failure_is_reported_and_the_sample_loop_keeps_going(self) -> None:
        # `sample_loop` wraps each iteration in a broad handler precisely so one bad frame cannot end the
        # session. Raising from `infer()` drives that path: the loop must surface the error via status + log
        # and still run every remaining iteration rather than propagating out of `run`.
        platform = FailingInferencePlatform(stop_after=3)
        with patch.object(eye_detection, "ANALYSIS_INTERVAL_MS", 0.0):
            await run(platform)

        self.assertEqual(platform.infer_calls, 3, "every iteration must still run after a failing one")
        self.assertTrue(
            any("inference error" in line and "inference exploded" in line for line in platform.logs),
            f"expected the raised error logged, got {platform.logs}",
        )
        self.assertTrue(
            any("inference error" in status for status in platform.statuses),
            f"expected the error surfaced as status, got {platform.statuses}",
        )
        self.assertTrue(platform.cleaned, "cleanup must still run after inference failures")

    async def test_eye_capture_failure_is_logged_and_does_not_abort_the_run(self) -> None:
        platform = FailingCapturePlatform(stop_after=5)
        platform.consent = True
        # not-detected, detected (edge #1 -> attempt+fail), not-detected (clears), detected (edge #2 ->
        # attempt+fail), not-detected. Two rising edges -> two failed attempts, each independently retried
        # rather than the first failure suppressing the second episode's attempt.
        analyses = [
            fake_analysis(),
            fake_analysis(misalignment=True),
            fake_analysis(),
            fake_analysis(misalignment=True),
            fake_analysis(),
        ]
        with (
            patch.object(eye_detection, "ANALYSIS_INTERVAL_MS", 0.0),
            patch.object(eye_detection, "analyze_window", side_effect=analyses),
        ):
            await run(platform)
        self.assertEqual(platform.capture_calls, 2)
        failure_logs = [line for line in platform.logs if "eye capture failed" in line and "upload failed" in line]
        self.assertEqual(len(failure_logs), 2)
        self.assertEqual(platform.statuses[-1], stopped_status())

    async def test_periodic_capture_fires_every_sample_when_its_interval_is_forced_to_zero(self) -> None:
        # Detection interval is left huge so it can never tick, isolating the periodic mechanism: every
        # sample is a periodic-capture tick, and no screening indicator ever fires.
        platform = FakePlatform(stop_after=5)
        platform.consent = True
        with (
            patch.object(eye_detection, "ANALYSIS_INTERVAL_MS", 999_999_999.0),
            patch.object(eye_detection, "PERIODIC_CAPTURE_INTERVAL_MS", 0.0),
        ):
            await run(platform)
        self.assertEqual(platform.capture_calls, 5)

    async def test_periodic_capture_is_also_gated_on_consent(self) -> None:
        platform = FakePlatform(stop_after=5)
        with (
            patch.object(eye_detection, "ANALYSIS_INTERVAL_MS", 999_999_999.0),
            patch.object(eye_detection, "PERIODIC_CAPTURE_INTERVAL_MS", 0.0),
        ):
            await run(platform)
        self.assertEqual(platform.capture_calls, 0)

    async def test_periodic_and_detection_triggered_captures_are_additive(self) -> None:
        # The indicator fires once (rising edge on the first sample, then stays active) while the periodic
        # interval is forced to zero: the detection edge contributes one capture, the periodic heartbeat
        # contributes one per sample, and the two mechanisms don't suppress each other.
        platform = FakePlatform(stop_after=5)
        platform.consent = True
        with (
            patch.object(eye_detection, "ANALYSIS_INTERVAL_MS", 0.0),
            patch.object(eye_detection, "PERIODIC_CAPTURE_INTERVAL_MS", 0.0),
            patch.object(eye_detection, "analyze_window", return_value=fake_analysis(misalignment=True)),
        ):
            await run(platform)
        self.assertEqual(platform.capture_calls, 6)

    async def test_eye_capture_failure_is_also_reported_as_a_server_visible_event(self) -> None:
        # platform.log() alone only reaches the browser's own log box; a failure must also be sent as a
        # client-event so it's visible server-side without anyone having to watch the browser.
        platform = FailingCapturePlatform(stop_after=5)
        platform.consent = True
        with (
            patch.object(eye_detection, "ANALYSIS_INTERVAL_MS", 0.0),
            patch.object(eye_detection, "analyze_window", return_value=fake_analysis(oscillation=True)),
        ):
            await run(platform)

        failure_events = [json.loads(event) for event in platform.events if "eye_capture_failed" in event]
        self.assertEqual(len(failure_events), 1)
        self.assertEqual(failure_events[0]["capability"], "pyeye1")
        self.assertEqual(failure_events[0]["action"], "eye_capture_failed")
        self.assertEqual(failure_events[0]["details"]["error"], "upload failed")


if __name__ == "__main__":
    unittest.main()
