import math
import unittest

from pyeye1.gaze_analysis import (
    LEFT_EYE_AXIS,
    LEFT_EYE_LIDS,
    LEFT_IRIS_CENTER,
    LEFT_IRIS_RING,
    MESH_LANDMARK_COUNT,
    MIN_ANALYSIS_SAMPLES,
    RIGHT_EYE_AXIS,
    RIGHT_EYE_LIDS,
    RIGHT_IRIS_CENTER,
    RIGHT_IRIS_RING,
    GazeSample,
    analyze_window,
    gaze_sample,
    misalignment_metrics,
    oscillation_metrics,
)


def landmarks_with(points: dict[int, tuple[float, float]]) -> list[float]:
    """Build a flat 478*2 normalized landmark list, placing (x, y) at the given landmark indices."""
    values = [0.0] * (MESH_LANDMARK_COUNT * 2)
    for index, (x, y) in points.items():
        values[index * 2] = x
        values[index * 2 + 1] = y
    return values


def face_with_gaze(right_iris: tuple[float, float], left_iris: tuple[float, float]) -> list[float]:
    """Build a face whose eyes sit on fixed horizontal axes with open lids and the given iris centers."""
    return landmarks_with(
        {
            RIGHT_EYE_AXIS[0]: (0.30, 0.50),
            RIGHT_EYE_AXIS[1]: (0.40, 0.50),
            RIGHT_EYE_LIDS[0]: (0.35, 0.48),
            RIGHT_EYE_LIDS[1]: (0.35, 0.52),
            RIGHT_IRIS_CENTER: right_iris,
            LEFT_EYE_AXIS[0]: (0.60, 0.50),
            LEFT_EYE_AXIS[1]: (0.70, 0.50),
            LEFT_EYE_LIDS[0]: (0.65, 0.48),
            LEFT_EYE_LIDS[1]: (0.65, 0.52),
            LEFT_IRIS_CENTER: left_iris,
        }
    )


def sample(
    t: float,
    left_h: float,
    right_h: float,
    left_v: float = 0.5,
    right_v: float = 0.5,
    valid: bool = True,
) -> GazeSample:
    """Build one analysis-level gaze sample without going through landmark decoding."""
    return {
        "t": t,
        "left": {"h": left_h, "v": left_v, "openness": 0.4},
        "right": {"h": right_h, "v": right_v, "openness": 0.4},
        "valid": valid,
    }


class GazeSampleTests(unittest.TestCase):
    def test_iris_index_sets_are_within_the_refined_mesh(self) -> None:
        indices = (RIGHT_IRIS_CENTER, *RIGHT_IRIS_RING, LEFT_IRIS_CENTER, *LEFT_IRIS_RING)
        self.assertEqual(len(indices), 10)
        for index in indices:
            self.assertGreaterEqual(index, 468)
            self.assertLess(index, MESH_LANDMARK_COUNT)

    def test_centered_iris_yields_half_ratios(self) -> None:
        face = face_with_gaze(right_iris=(0.35, 0.50), left_iris=(0.65, 0.50))
        result = gaze_sample(face, 1000.0, 1000.0, 1.25)
        self.assertAlmostEqual(result["right"]["h"], 0.5)
        self.assertAlmostEqual(result["left"]["h"], 0.5)
        self.assertAlmostEqual(result["right"]["v"], 0.5)
        # Lid gap 0.04 over eye width 0.10 -> openness 0.4, comfortably above the blink floor.
        self.assertAlmostEqual(result["right"]["openness"], 0.4)
        self.assertTrue(result["valid"])
        self.assertAlmostEqual(result["t"], 1.25)

    def test_off_center_iris_yields_proportional_ratio(self) -> None:
        face = face_with_gaze(right_iris=(0.375, 0.50), left_iris=(0.65, 0.50))
        result = gaze_sample(face, 640.0, 480.0, 0.0)
        self.assertAlmostEqual(result["right"]["h"], 0.75)

    def test_head_roll_diagonal_axis_still_centers(self) -> None:
        # Rotate the right eye 45 degrees: the projection onto its own axis keeps the centered ratio at 0.5.
        face = landmarks_with(
            {
                RIGHT_EYE_AXIS[0]: (0.30, 0.30),
                RIGHT_EYE_AXIS[1]: (0.40, 0.40),
                RIGHT_EYE_LIDS[0]: (0.36, 0.34),
                RIGHT_EYE_LIDS[1]: (0.34, 0.36),
                RIGHT_IRIS_CENTER: (0.35, 0.35),
                LEFT_EYE_AXIS[0]: (0.60, 0.50),
                LEFT_EYE_AXIS[1]: (0.70, 0.50),
                LEFT_EYE_LIDS[0]: (0.65, 0.48),
                LEFT_EYE_LIDS[1]: (0.65, 0.52),
                LEFT_IRIS_CENTER: (0.65, 0.50),
            }
        )
        result = gaze_sample(face, 1000.0, 1000.0, 0.0)
        self.assertAlmostEqual(result["right"]["h"], 0.5)

    def test_closed_lids_invalidate_the_sample(self) -> None:
        face = face_with_gaze(right_iris=(0.35, 0.50), left_iris=(0.65, 0.50))
        for index in (*RIGHT_EYE_LIDS, *LEFT_EYE_LIDS):
            face[index * 2] = 0.35
            face[index * 2 + 1] = 0.50
        result = gaze_sample(face, 1000.0, 1000.0, 0.0)
        self.assertFalse(result["valid"])
        self.assertAlmostEqual(result["right"]["v"], 0.5)

    def test_rejects_landmarks_without_iris_points(self) -> None:
        with self.assertRaisesRegex(ValueError, "iris landmarks"):
            gaze_sample([0.0] * (468 * 2), 640.0, 480.0, 0.0)


class MisalignmentTests(unittest.TestCase):
    def test_sustained_horizontal_offset_is_detected(self) -> None:
        samples = [sample(i * 0.05, left_h=0.50, right_h=0.35) for i in range(30)]
        metrics = misalignment_metrics(samples)
        self.assertEqual(metrics["status"], "ok")
        self.assertTrue(metrics["detected"])
        self.assertAlmostEqual(metrics["horizontal_deviation"] or 0.0, 0.15)

    def test_sustained_vertical_offset_is_detected(self) -> None:
        samples = [sample(i * 0.05, left_h=0.50, right_h=0.50, left_v=0.62, right_v=0.45) for i in range(30)]
        metrics = misalignment_metrics(samples)
        self.assertTrue(metrics["detected"])
        self.assertAlmostEqual(metrics["vertical_deviation"] or 0.0, 0.17)

    def test_conjugate_gaze_shift_is_not_misalignment(self) -> None:
        # Both eyes sweep together from 0.3 to 0.7: a large gaze movement but zero left-right difference.
        samples = [sample(i * 0.05, left_h=0.3 + i * 0.4 / 29, right_h=0.3 + i * 0.4 / 29) for i in range(30)]
        metrics = misalignment_metrics(samples)
        self.assertEqual(metrics["status"], "ok")
        self.assertFalse(metrics["detected"])

    def test_too_few_valid_samples_is_insufficient_data(self) -> None:
        samples = [sample(i * 0.05, 0.5, 0.3, valid=i < 5) for i in range(MIN_ANALYSIS_SAMPLES + 5)]
        metrics = misalignment_metrics(samples)
        self.assertEqual(metrics["status"], "insufficient_data")
        self.assertFalse(metrics["detected"])
        self.assertIsNone(metrics["horizontal_deviation"])


class OscillationTests(unittest.TestCase):
    def test_conjugate_oscillation_is_detected(self) -> None:
        # A 4 Hz, 0.08-amplitude horizontal oscillation of both eyes, sampled at 20 Hz for 2.4 seconds.
        samples = []
        for i in range(48):
            t = i * 0.05
            h = 0.5 + 0.08 * math.sin(2.0 * math.pi * 4.0 * t)
            samples.append(sample(t, left_h=h, right_h=h))
        metrics = oscillation_metrics(samples)
        self.assertEqual(metrics["status"], "ok")
        self.assertTrue(metrics["detected"])
        self.assertEqual(metrics["axis"], "horizontal")
        self.assertGreaterEqual(metrics["frequency_hz"] or 0.0, 3.0)
        self.assertLessEqual(metrics["frequency_hz"] or 0.0, 5.0)
        self.assertGreaterEqual(metrics["amplitude"] or 0.0, 0.04)

    def test_vertical_oscillation_reports_the_vertical_axis(self) -> None:
        samples = []
        for i in range(48):
            t = i * 0.05
            v = 0.5 + 0.08 * math.sin(2.0 * math.pi * 4.0 * t)
            samples.append(sample(t, left_h=0.5, right_h=0.5, left_v=v, right_v=v))
        metrics = oscillation_metrics(samples)
        self.assertTrue(metrics["detected"])
        self.assertEqual(metrics["axis"], "vertical")

    def test_steady_gaze_is_not_detected(self) -> None:
        samples = [sample(i * 0.05, left_h=0.5, right_h=0.5) for i in range(48)]
        metrics = oscillation_metrics(samples)
        self.assertEqual(metrics["status"], "ok")
        self.assertFalse(metrics["detected"])
        self.assertAlmostEqual(metrics["amplitude"] or 0.0, 0.0)

    def test_slow_smooth_pursuit_is_not_detected(self) -> None:
        # A steady drift across the eye is pursuit / head motion: detrending leaves almost no residual.
        samples = [sample(i * 0.05, left_h=0.4 + i * 0.2 / 47, right_h=0.4 + i * 0.2 / 47) for i in range(48)]
        metrics = oscillation_metrics(samples)
        self.assertEqual(metrics["status"], "ok")
        self.assertFalse(metrics["detected"])

    def test_short_window_is_insufficient_data(self) -> None:
        samples = [sample(i * 0.05, left_h=0.5, right_h=0.5) for i in range(MIN_ANALYSIS_SAMPLES)]
        metrics = oscillation_metrics(samples)
        self.assertEqual(metrics["status"], "insufficient_data")
        self.assertFalse(metrics["detected"])


class AnalyzeWindowTests(unittest.TestCase):
    def test_empty_window_reports_zero_counts(self) -> None:
        analysis = analyze_window([])
        self.assertEqual(analysis["samples"], 0)
        self.assertEqual(analysis["valid_samples"], 0)
        self.assertEqual(analysis["window_ms"], 0.0)
        self.assertEqual(analysis["misalignment"]["status"], "insufficient_data")
        self.assertEqual(analysis["oscillation"]["status"], "insufficient_data")

    def test_window_counts_and_span(self) -> None:
        samples = [sample(i * 0.05, 0.5, 0.5, valid=i % 2 == 0) for i in range(40)]
        analysis = analyze_window(samples)
        self.assertEqual(analysis["samples"], 40)
        self.assertEqual(analysis["valid_samples"], 20)
        self.assertAlmostEqual(analysis["window_ms"], 39 * 50.0, places=1)


if __name__ == "__main__":
    unittest.main()
