import math
import unittest

from pyface1.face_detection import (
    FACE_INPUT_HEIGHT,
    FACE_INPUT_WIDTH,
    build_priors,
    decode_box,
    decode_outputs,
    output_values,
    preprocess_geometry,
    softmax,
)


class RetinaFaceTests(unittest.TestCase):
    def test_softmax_handles_empty_equal_and_large_values(self) -> None:
        self.assertEqual(softmax([]), [])

        equal = softmax([4.0, 4.0, 4.0, 4.0])
        self.assertTrue(all(abs(value - 0.25) < 1e-6 for value in equal))

        large = softmax([1000.0, 1001.0])
        self.assertEqual(len(large), 2)
        self.assertTrue(all(math.isfinite(value) for value in large))
        self.assertAlmostEqual(sum(large), 1.0, places=6)
        self.assertGreater(large[1], large[0])

    def test_prior_count_matches_model_input_shape(self) -> None:
        priors = build_priors(float(FACE_INPUT_HEIGHT), float(FACE_INPUT_WIDTH))

        self.assertEqual(len(priors), 15_960)
        self.assertAlmostEqual(priors[0][0], 4.0 / FACE_INPUT_WIDTH, places=6)
        self.assertAlmostEqual(priors[0][1], 4.0 / FACE_INPUT_HEIGHT, places=6)
        self.assertAlmostEqual(priors[0][2], 16.0 / FACE_INPUT_WIDTH, places=6)
        self.assertAlmostEqual(priors[0][3], 16.0 / FACE_INPUT_HEIGHT, places=6)

    def test_zero_offsets_decode_to_prior_box(self) -> None:
        decoded = decode_box([0.0, 0.0, 0.0, 0.0], [0.5, 0.5, 0.25, 0.5])

        self.assertAlmostEqual(decoded[0], 0.375, places=6)
        self.assertAlmostEqual(decoded[1], 0.25, places=6)
        self.assertAlmostEqual(decoded[2], 0.625, places=6)
        self.assertAlmostEqual(decoded[3], 0.75, places=6)

    def test_output_values_rejects_trailing_shape_data(self) -> None:
        with self.assertRaisesRegex(ValueError, "unexpected shapes"):
            output_values([0.0, 1.0, 2.0], "loc", 4)

    def test_decode_outputs_rejects_invalid_resize_ratio(self) -> None:
        loc = [0.0] * (15_960 * 4)
        conf = [0.0] * (15_960 * 2)
        landm = [0.0] * (15_960 * 10)

        with self.assertRaisesRegex(ValueError, "resize_ratio"):
            decode_outputs(loc, conf, landm, 0.0, 640.0, 480.0)

    def test_preprocess_geometry_preserves_source_aspect_ratio(self) -> None:
        wide = preprocess_geometry(1280.0, 720.0)
        self.assertAlmostEqual(wide["resize_ratio"], FACE_INPUT_WIDTH / 1280.0)
        self.assertEqual(wide["resized_width"], 640.0)
        self.assertEqual(wide["resized_height"], 360.0)

        tall = preprocess_geometry(480.0, 960.0)
        self.assertAlmostEqual(tall["resize_ratio"], FACE_INPUT_HEIGHT / 960.0)
        self.assertEqual(tall["resized_width"], 304.0)
        self.assertEqual(tall["resized_height"], 608.0)


if __name__ == "__main__":
    unittest.main()
