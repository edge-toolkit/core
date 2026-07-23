import json
import unittest
from unittest.mock import patch

from pydemo1 import config, eye_capture_error_json, process_eye_capture, process_speech_capture, reset_eye_capture, run
from pydemo1.demo import EyeCaptureProcessor


class FakePlatform:
    def __init__(self) -> None:
        self.state = "connecting"
        self.calls: list[str] = []
        self.cleaned = False

    def set_loading_message(self, message: str) -> None:
        self.calls.append(message)

    async def connect_ws(self) -> None:
        self.calls.append("connect")

    def ws_state(self) -> str:
        return self.state

    def agent_id(self) -> str:
        return "fake-agent"

    async def load_models(self) -> None:
        self.calls.append("models")

    def show_demo(self) -> None:
        self.calls.append("show")

    async def capture(self) -> None:
        self.calls.append("capture")

    def should_stop(self) -> bool:
        return False

    def cleanup(self) -> None:
        self.cleaned = True

    async def sleep(self, _milliseconds: int) -> None:
        self.state = "connected"

    def log(self, message: str) -> None:
        self.calls.append(message)


class DemoTests(unittest.TestCase):
    def test_config_reuses_existing_module_assets(self) -> None:
        value = config()
        self.assertEqual(value["speech"]["capture_seconds"], 30)
        self.assertIn("face_landmarker.task", value["eye"]["model_path"])
        self.assertIn("speech1.onnx", value["speech"]["model_path"])

    def test_empty_eye_capture_has_no_detections(self) -> None:
        reset_eye_capture()
        result = process_eye_capture(json.dumps({"faces": [], "width": 1280, "height": 720}))
        self.assertEqual(result["face_count"], 0)
        self.assertEqual(result["eye_count"], 0)
        self.assertEqual(result["capture_count"], 0)
        payload = json.loads(result["results_json"])
        self.assertEqual(payload["faces"], [])
        self.assertIsNone(payload["analysis"])
        self.assertIsNone(payload["crop"])

    def test_speech_capture_reuses_speech_classifier(self) -> None:
        result = process_speech_capture([0.9] * 10, 48_000, 30.0)
        self.assertTrue(result["speech_detected"])
        self.assertAlmostEqual(result["confidence"], 0.9)
        event = json.loads(result["event_json"])
        self.assertEqual(event["capability"], "speech_detection")

    def test_consented_eye_capture_is_requested_on_periodic_interval(self) -> None:
        with patch("pydemo1.demo.time.monotonic", side_effect=[0.0, 5.1]):
            processor = EyeCaptureProcessor()
            result = processor.process({"faces": [], "width": 1280, "height": 720, "upload_consent": True})
        self.assertEqual(result["capture_count"], 1)

    def test_eye_capture_error_reuses_pyeye1_event(self) -> None:
        event = json.loads(eye_capture_error_json("upload failed"))
        self.assertEqual(event["capability"], "pyeye1")
        self.assertEqual(event["action"], "eye_capture_failed")


class DemoRunTests(unittest.IsolatedAsyncioTestCase):
    async def test_run_drives_workflow_and_always_cleans_up(self) -> None:
        platform = FakePlatform()
        await run(platform)
        self.assertIn("connect", platform.calls)
        self.assertIn("models", platform.calls)
        self.assertIn("show", platform.calls)
        self.assertIn("capture", platform.calls)
        self.assertTrue(platform.cleaned)

    async def test_capture_failure_still_cleans_up(self) -> None:
        platform = FakePlatform()

        async def fail_capture() -> None:
            raise RuntimeError("capture failed")

        platform.capture = fail_capture  # type: ignore[method-assign]
        with self.assertRaisesRegex(RuntimeError, "capture failed"):
            await run(platform)
        self.assertTrue(platform.cleaned)


if __name__ == "__main__":
    unittest.main()
