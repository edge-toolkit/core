import json
import unittest

from pyspeech1.speech_detection import (
    CHUNK_SIZE,
    SAMPLE_RATE,
    client_event_json,
    config,
    event_payload,
    summarize_probabilities,
)


class SpeechDetectionTests(unittest.TestCase):
    def test_config_matches_speech1_inputs(self) -> None:
        self.assertEqual(config()["sample_rate"], 16_000)
        self.assertEqual(config()["chunk_size"], 512)
        self.assertEqual(config()["context_size"], 64)

    def test_silence_is_not_speech(self) -> None:
        result = summarize_probabilities([0.01] * 20)
        self.assertFalse(result["speech_detected"])
        self.assertEqual(result["speech_duration_ms"], 0)

    def test_sustained_speech_is_detected(self) -> None:
        result = summarize_probabilities([0.02] * 3 + [0.91] * 10 + [0.01] * 4)
        self.assertTrue(result["speech_detected"])
        self.assertGreaterEqual(result["speech_duration_ms"], 250)
        self.assertAlmostEqual(result["confidence"], 0.91)

    def test_short_noise_spike_is_rejected(self) -> None:
        result = summarize_probabilities([0.01] * 3 + [0.99] * 3 + [0.01] * 4)
        self.assertFalse(result["speech_detected"])

    def test_invalid_or_empty_output_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "no probabilities"):
            summarize_probabilities([])
        with self.assertRaisesRegex(ValueError, "invalid"):
            summarize_probabilities([float("nan")])

    def test_event_uses_speech_detection_capability(self) -> None:
        summary = summarize_probabilities([0.9] * 10)
        payload = event_payload(summary, 48_000, 5.0)
        event = json.loads(client_event_json(payload))
        self.assertEqual(event["capability"], "speech_detection")
        self.assertEqual(event["action"], "inference")
        self.assertEqual(event["details"]["label"], "speech")
        self.assertEqual(event["details"]["model_sample_rate"], SAMPLE_RATE)
        self.assertEqual(CHUNK_SIZE, 512)


if __name__ == "__main__":
    unittest.main()
