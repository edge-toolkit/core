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


class NeverConnectingPlatform(FakePlatform):
    def ws_state(self) -> str:
        return "connecting"


class CameraDeniedPlatform(FakePlatform):
    async def start_camera(self) -> None:
        raise RuntimeError("Permission denied")


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


if __name__ == "__main__":
    unittest.main()
