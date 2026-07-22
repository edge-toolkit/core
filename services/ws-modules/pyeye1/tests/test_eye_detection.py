import unittest

from pyeye1.eye_detection import (
    CONTOUR_LANDMARK_COUNT,
    LEFT_EYE_INDICES,
    MESH_LANDMARK_COUNT,
    RIGHT_EYE_INDICES,
    FaceEyes,
    build_results,
    decode_eye_boxes,
    decode_irises,
    eye_region_crop,
    face_bounds,
    smooth_crop,
)
from pyeye1.gaze_analysis import RIGHT_IRIS_CENTER, RIGHT_IRIS_RING


def face_eyes(face_box: list[float], left_eye: list[float], right_eye: list[float]) -> FaceEyes:
    """Build a FaceEyes result directly from boxes, skipping landmark decoding."""
    return {
        "face_box": face_box,
        "eyes": [
            {"label": "left_eye", "box": left_eye},
            {"label": "right_eye", "box": right_eye},
        ],
        "irises": [],
    }


def landmarks_with(points: dict[int, tuple[float, float]]) -> list[float]:
    """Build a flat 478*2 normalized landmark list, placing (x, y) at the given landmark indices."""
    values = [0.0] * (MESH_LANDMARK_COUNT * 2)
    for index, (x, y) in points.items():
        values[index * 2] = x
        values[index * 2 + 1] = y
    return values


class EyeDetectionTests(unittest.TestCase):
    def test_eye_index_sets_are_within_the_mesh(self) -> None:
        self.assertEqual(len(RIGHT_EYE_INDICES), 16)
        self.assertEqual(len(LEFT_EYE_INDICES), 16)
        for index in (*RIGHT_EYE_INDICES, *LEFT_EYE_INDICES):
            self.assertGreaterEqual(index, 0)
            self.assertLess(index, CONTOUR_LANDMARK_COUNT)

    def test_decode_eye_boxes_maps_normalized_landmarks_to_source_pixels(self) -> None:
        first = RIGHT_EYE_INDICES[0]
        last = RIGHT_EYE_INDICES[-1]
        # Place two right-eye contour points at normalized (0.25, 0.5) and (0.75, 0.5) in a 640x480 frame.
        values = landmarks_with({first: (0.25, 0.5), last: (0.75, 0.5)})
        # Give the left-eye cluster a non-zero location so its box isn't pinned at the origin.
        for index in LEFT_EYE_INDICES:
            values[index * 2] = 0.6
            values[index * 2 + 1] = 0.6

        boxes = decode_eye_boxes(values, 640.0, 480.0)
        right = boxes["right_eye"]
        # Other right-eye indices sit at 0 -> min stays at origin; the two placed points set the max.
        self.assertAlmostEqual(right[0], 0.0)
        self.assertAlmostEqual(right[2], 0.75 * 640.0)
        self.assertAlmostEqual(right[3], 0.5 * 480.0)
        self.assertAlmostEqual(boxes["left_eye"][0], 0.6 * 640.0)

    def test_decode_eye_boxes_clamps_out_of_range_coords(self) -> None:
        values = landmarks_with(dict.fromkeys(RIGHT_EYE_INDICES, (1.5, -0.2)))
        boxes = decode_eye_boxes(values, 100.0, 200.0)
        # 1.5 clamps to width 100, -0.2 clamps to 0.
        self.assertAlmostEqual(boxes["right_eye"][0], 100.0)
        self.assertAlmostEqual(boxes["right_eye"][2], 100.0)
        self.assertAlmostEqual(boxes["right_eye"][1], 0.0)

    def test_decode_eye_boxes_rejects_short_output(self) -> None:
        with self.assertRaisesRegex(ValueError, "face-mesh landmarks"):
            decode_eye_boxes([0.0] * 10, 640.0, 480.0)

    def test_decode_eye_boxes_rejects_bad_dimensions(self) -> None:
        with self.assertRaisesRegex(ValueError, "image_width"):
            decode_eye_boxes(landmarks_with({}), 0.0, 480.0)

    def test_face_bounds_covers_all_landmarks(self) -> None:
        values = landmarks_with({10: (0.1, 0.2), 20: (0.9, 0.8)})
        bounds = face_bounds(values, 1000.0, 500.0)
        self.assertAlmostEqual(bounds[0], 0.0)
        self.assertAlmostEqual(bounds[2], 0.9 * 1000.0)
        self.assertAlmostEqual(bounds[3], 0.8 * 500.0)

    def test_build_results_pairs_each_face_with_two_eyes(self) -> None:
        faces = [landmarks_with(dict.fromkeys((*RIGHT_EYE_INDICES, *LEFT_EYE_INDICES), (0.5, 0.5)))]
        results = build_results(faces, 640.0, 480.0)
        self.assertEqual(len(results), 1)
        self.assertEqual([eye["label"] for eye in results[0]["eyes"]], ["left_eye", "right_eye"])
        self.assertEqual(len(results[0]["face_box"]), 4)
        self.assertEqual([iris["label"] for iris in results[0]["irises"]], ["left_eye", "right_eye"])

    def test_decode_irises_maps_center_and_ring_to_a_pixel_circle(self) -> None:
        # Center at (0.35, 0.50) with the ring 0.01 out on each side; in a 1000x500 frame the horizontal ring
        # points sit 10px from the center and the vertical ones 5px, so the radius averages to 7.5px.
        points = {RIGHT_IRIS_CENTER: (0.35, 0.50)}
        ring_offsets = ((0.01, 0.0), (0.0, -0.01), (-0.01, 0.0), (0.0, 0.01))
        for index, (dx, dy) in zip(RIGHT_IRIS_RING, ring_offsets, strict=True):
            points[index] = (0.35 + dx, 0.50 + dy)
        irises = decode_irises(landmarks_with(points), 1000.0, 500.0)
        right = next(iris for iris in irises if iris["label"] == "right_eye")
        self.assertAlmostEqual(right["center"][0], 350.0)
        self.assertAlmostEqual(right["center"][1], 250.0)
        self.assertAlmostEqual(right["radius"], 7.5)

    def test_decode_irises_rejects_contour_only_landmarks(self) -> None:
        with self.assertRaisesRegex(ValueError, "iris landmarks"):
            decode_irises([0.0] * (CONTOUR_LANDMARK_COUNT * 2), 640.0, 480.0)

    def test_eye_region_crop_is_a_face_wide_band_centered_on_the_eyes(self) -> None:
        result = face_eyes(
            face_box=[100.0, 50.0, 300.0, 350.0],
            left_eye=[220.0, 150.0, 260.0, 170.0],
            right_eye=[140.0, 150.0, 180.0, 168.0],
        )
        crop = eye_region_crop(result, 640.0, 480.0)
        # Face width 200 -> 4% margin of 8 each side; eye band 150..170 centers at 160, and the half-height
        # is 22% of the 300px face height (66), which beats the 20px eye-cluster floor.
        self.assertEqual(crop, [92.0, 94.0, 308.0, 226.0])

    def test_eye_region_crop_clamps_to_the_frame(self) -> None:
        result = face_eyes(
            face_box=[0.0, 0.0, 200.0, 200.0],
            left_eye=[120.0, 40.0, 160.0, 55.0],
            right_eye=[20.0, 40.0, 60.0, 55.0],
        )
        crop = eye_region_crop(result, 205.0, 90.0)
        self.assertEqual(crop[0], 0.0)
        self.assertEqual(crop[2], 205.0)
        self.assertAlmostEqual(crop[1], 3.5)
        self.assertEqual(crop[3], 90.0)

    def test_smooth_crop_starts_at_the_target_then_eases_toward_it(self) -> None:
        target = [10.0, 10.0, 110.0, 110.0]
        self.assertEqual(smooth_crop(None, target), target)
        eased = smooth_crop([0.0, 0.0, 100.0, 100.0], target)
        self.assertEqual(eased, [3.5, 3.5, 103.5, 103.5])


if __name__ == "__main__":
    unittest.main()
