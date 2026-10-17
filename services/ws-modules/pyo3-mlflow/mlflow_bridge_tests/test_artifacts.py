"""Black-box tests for MLflow bridge artifacts."""

import json
import os
import unittest
from unittest.mock import patch

from .support import STATE_FILE, BridgeFixtureMixin, make_event, wait_until


class ArtifactsTests(BridgeFixtureMixin, unittest.TestCase):
    """Exercise bridge artifacts through its public lifecycle hooks."""

    def test_artifact_storage_is_denied_without_operator_allowlist(self):
        """Reject producer-controlled reads without accessing the requested agent's storage."""
        self.connect()
        artifact = {"bucket": "victim", "filename": "private.json", "artifact_path": "leaked.json"}
        self.module.on_text_frame(json.dumps(make_event(event="artifacts", artifacts=[artifact])))
        self.assertEqual(self.sender.messages[-1]["status"], "rejected")
        self.assertNotIn(("get", "victim", "private.json"), self.timeline)
        self.assertEqual(self.client.artifacts, [])

    def test_artifacts_cannot_read_bridge_storage_even_if_allowlisted(self):
        """Protect the bridge state from a producer even when its bucket is mistakenly allowed."""
        with patch.dict(os.environ, {"PYO3_MLFLOW_ARTIFACT_BUCKETS": "mlflow-agent"}):
            self.connect()
            artifact = {"bucket": "mlflow-agent", "filename": STATE_FILE, "artifact_path": "state.json"}
            self.module.on_text_frame(json.dumps(make_event(event="artifacts", artifacts=[artifact])))
            self.assertEqual(self.sender.messages[-1]["status"], "rejected")
            self.assertEqual(self.client.artifacts, [])

    def test_operator_allowed_artifact_is_uploaded(self):
        """Read an explicitly shared agent bucket and upload its artifact payload."""
        settings = {"PYO3_MLFLOW_ARTIFACT_BUCKETS": " shared-agent ", "PYO3_MLFLOW_MAX_ARTIFACT_BYTES": "abc"}
        with patch.dict(os.environ, settings):
            self.storage.remote_values[("shared-agent", "matrix.json")] = b"matrix"
            self.connect()
            artifact = {"bucket": "shared-agent", "filename": "matrix.json", "artifact_path": "plots/matrix.json"}
            self.module.on_text_frame(json.dumps(make_event(event="artifacts", artifacts=[artifact])))
            wait_until(lambda: any(message["status"] == "logged" for message in self.sender.messages))
        self.assertEqual(self.client.artifacts, [("run-1", b"matrix", "plots")])

    def test_replayed_artifact_is_checked_against_current_policy(self):
        """Prevent a previously queued artifact from bypassing a changed storage policy."""
        artifact = {"bucket": "previously-shared", "filename": "private.json", "artifact_path": "private.json"}
        self.persist_pending(make_event(event="artifacts", artifacts=[artifact]))
        with (
            patch.dict(os.environ, {"PYO3_MLFLOW_MAX_RETRIES": "1"}),
            self.assertLogs(self.module.__name__, level="ERROR"),
        ):
            self.connect()
            wait_until(lambda: any(message["status"] == "failed" for message in self.sender.messages))
        self.assertNotIn(("get", "previously-shared", "private.json"), self.timeline)
        self.assertEqual(self.client.artifacts, [])

    def test_missing_and_oversized_artifacts_fail_without_uploading(self):
        """Report artifact retrieval and size-limit failures without uploading partial results."""
        settings = {
            "PYO3_MLFLOW_ARTIFACT_BUCKETS": "shared",
            "PYO3_MLFLOW_MAX_ARTIFACT_BYTES": "3",
            "PYO3_MLFLOW_MAX_RETRIES": "1",
        }
        self.storage.remote_values[("shared", "large.bin")] = b"too large"
        with patch.dict(os.environ, settings):
            self.connect()
            for filename in ("missing.bin", "large.bin"):
                with self.subTest(filename=filename), self.assertLogs(self.module.__name__, level="ERROR"):
                    artifact = {"bucket": "shared", "filename": filename, "artifact_path": filename}
                    event = make_event(event="artifacts", event_id=filename, artifacts=[artifact])
                    self.module.on_text_frame(json.dumps(event))
                    self.wait_for_status("failed", filename)
        self.assertEqual(self.client.artifacts, [])
        self.assertEqual(json.loads(self.storage.values[STATE_FILE])["pending"], [])
