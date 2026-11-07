"""Black-box tests for MLflow bridge validation."""

import json
import os
import unittest
from unittest.mock import patch

from .support import STATE_FILE, BridgeFixtureMixin, make_event, wait_until


class ValidationTests(BridgeFixtureMixin, unittest.TestCase):
    """Exercise bridge validation through its public lifecycle hooks."""

    def test_schema_one_requires_producer(self):
        """Reject a schema-v1 frame that omits the producer namespace."""
        self.connect()
        value = make_event()
        value.pop("producer")
        self.module.on_text_frame(json.dumps(value))
        self.assertEqual(self.sender.messages[-1]["status"], "rejected")

    def test_rejects_unknown_schema_version(self):
        """Reject a frame from a protocol version the bridge does not implement."""
        self.connect()
        self.module.on_text_frame(json.dumps(make_event(schema_version=999)))
        self.assertEqual(self.sender.messages[-1]["status"], "rejected")

    def test_invalid_numeric_environment_does_not_break_hooks(self):
        """Use defaults for malformed limits instead of failing connection and ingestion hooks."""
        settings = {
            "PYO3_MLFLOW_QUEUE_SIZE": "abc",
            "PYO3_MLFLOW_MAX_EVENT_BYTES": "",
            "PYO3_MLFLOW_DEDUP_HISTORY": "1.5",
        }
        with patch.dict(os.environ, settings):
            self.connect()
            self.module.on_text_frame(json.dumps(make_event()))
            wait_until(lambda: any(message["status"] == "logged" for message in self.sender.messages))
        self.assertEqual(len(self.client.metrics), 1)

    def test_unrelated_and_malformed_frames_are_ignored(self):
        """Ignore invalid JSON and non-tracking messages without creating runs or acknowledging them."""
        self.connect()
        for payload in ("not json", "[]", "null", '{"type":"other-event"}'):
            with self.subTest(payload=payload):
                self.module.on_text_frame(payload)
        self.assertEqual(self.sender.messages, [])
        self.assertEqual(self.client.created_runs, [])
        self.assertNotIn(STATE_FILE, self.storage.values)

    def test_oversized_frames_are_not_queued(self):
        """Apply the configured byte limit before processing incoming tracking events."""
        with patch.dict(os.environ, {"PYO3_MLFLOW_MAX_EVENT_BYTES": "16"}):
            self.connect()
            self.module.on_text_frame(json.dumps(make_event()))
        self.assertEqual(self.sender.messages, [])
        self.assertNotIn(STATE_FILE, self.storage.values)

    def test_invalid_event_payloads_are_rejected_before_persistence(self):
        """Validate operation kinds, names, mappings, metrics, steps and artifact pointers at ingestion."""
        cases = [
            {"event": "unknown-operation"},
            {"producer": None},
            {"producer": "../other-agent"},
            {"experiment": "invalid\nname"},
            {"run_name": []},
            {"event": "params", "params": []},
            {"event": "params", "params": {"nested": {"value": 1}}},
            {"event": "tags", "tags": {"temperature": float("inf")}},
            {"event": "run-start", "tags": []},
            {"metrics": []},
            {"metrics": {}},
            {"metrics": {"loss": True}},
            {"metrics": {"loss": "0.5"}},
            {"metrics": {"loss": float("nan")}},
            {"step": -1},
            {"step": True},
            {"step": 1.5},
            {"event": "run-end", "status": "UNKNOWN"},
            {"event": "artifacts", "artifacts": []},
            {"event": "artifacts", "artifacts": ["not an object"]},
            {"event": "artifacts", "artifacts": [{"bucket": "shared", "filename": "matrix.json"}]},
        ]
        with patch.dict(os.environ, {"PYO3_MLFLOW_ARTIFACT_BUCKETS": "shared"}):
            self.connect()
            for values in cases:
                with self.subTest(values=values):
                    message_count = len(self.sender.messages)
                    self.module.on_text_frame(json.dumps(make_event(**values)))
                    self.assertEqual(len(self.sender.messages), message_count + 1)
                    self.assertEqual(self.sender.messages[-1]["status"], "rejected")
        self.assertNotIn(STATE_FILE, self.storage.values)
        self.assertEqual(self.client.created_runs, [])
