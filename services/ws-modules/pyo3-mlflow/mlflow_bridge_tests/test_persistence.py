"""Persistence and recovery tests for the MLflow bridge."""

import json
import os
import sys
import threading
import time
import unittest
from unittest.mock import patch

from .support import STATE_FILE, BridgeFixtureMixin, make_event, timeline_index, wait_until


class PersistenceTests(BridgeFixtureMixin, unittest.TestCase):
    """Exercise persistence through public hooks, with one focused defensive-helper test."""

    def test_rescheduling_absent_event_leaves_pending_work_unchanged(self):
        """Ignore a stale retry without writing storage or disturbing a different pending event."""
        self.persist_pending(make_event())
        with self.paused_worker():
            saved_state = self.storage.values.copy()
            identity = "test-producer::run-1::absent"
            # The public worker selects pending events, so it cannot exercise this missing-record guard.
            # Direct access verifies that a stale retry neither writes state nor changes existing queued work.
            with patch.object(self.storage, "put", wraps=self.storage.put) as put:
                self.module._reschedule(identity, 3, time.time() + 60)  # skipcq: PYL-W0212 -- defensive guard test
            put.assert_not_called()
            self.assertEqual(self.storage.values, saved_state)
            self.assertEqual(self.sender.messages, [])
            self.assertEqual(self.client.metrics, [])
        self.wait_for_status("logged")
        self.assertEqual(len(self.client.metrics), 1)

    def test_persistence_failures_are_logged_and_not_acknowledged_as_queued(self):
        """Report storage failures without claiming that an event has been durably accepted."""
        with self.paused_worker():
            for index, error_type in enumerate((AttributeError, OSError, RuntimeError)):
                event_id = f"write-failure-{index}"
                with (
                    self.subTest(error_type=error_type.__name__),
                    patch.object(self.storage, "put", side_effect=error_type("storage write failed")),
                    self.assertLogs(self.module.__name__, level="ERROR") as captured,
                ):
                    self.module.on_text_frame(json.dumps(make_event(event_id=event_id)))
                messages = [message for message in self.sender.messages if message["event_id"] == event_id]
                self.assertEqual([message["status"] for message in messages], ["failed"])
                self.assertIn("outbox persistence failed: storage write failed", messages[0]["message"])
                self.assertIn("failed to persist MLflow event", "\n".join(captured.output))
                self.assertNotIn(STATE_FILE, self.storage.values)

    def test_failed_persistence_does_not_leave_an_event_for_background_processing(self):
        """Keep a rejected write out of memory and storage while subsequent healthy events are processed."""
        with self.paused_worker():
            with (
                patch.object(self.storage, "put", side_effect=OSError("storage unavailable")),
                self.assertLogs(self.module.__name__, level="ERROR"),
            ):
                self.module.on_text_frame(json.dumps(make_event(event_id="rejected-write", metrics={"rejected": 1.0})))
            self.module.on_text_frame(json.dumps(make_event(event_id="healthy-write", metrics={"healthy": 0.5})))
            document = json.loads(self.storage.values[STATE_FILE])
            self.assertEqual([item["event"]["event_id"] for item in document["pending"]], ["healthy-write"])
        self.wait_for_status("logged", "healthy-write")
        self.assertEqual([metric[1] for metric in self.client.metrics], ["healthy"])
        self.module.on_text_frame(json.dumps(make_event(event_id="rejected-write", metrics={"resubmitted": 1.0})))
        self.wait_for_status("logged", "rejected-write")
        self.assertEqual([metric[1] for metric in self.client.metrics], ["healthy", "resubmitted"])

    def test_completion_write_failure_restores_pending_work_and_history(self):
        """Preserve retry eligibility and prior deduplication history until completion is durably written."""
        failed_completion = threading.Event()
        saved_retry = threading.Event()
        release_retry = threading.Event()
        self.addCleanup(release_retry.set)
        original_put = self.storage.put

        def put(name, value):
            """Fail completion once and pause after saving the replacement retry record."""
            document = json.loads(bytes(value))
            if "test-producer::run-1::completion-retry" in document["processed"] and not failed_completion.is_set():
                failed_completion.set()
                raise OSError("completion write failed")
            original_put(name, value)
            if any(item["attempts"] == 1 for item in document["pending"]):
                saved_retry.set()
                if not release_retry.wait(timeout=5.0):
                    raise RuntimeError("test did not release retry persistence")

        settings = {"PYO3_MLFLOW_DEDUP_HISTORY": "1", "PYO3_MLFLOW_RETRY_BASE_SECONDS": "1"}
        with patch.dict(os.environ, settings):
            self.connect()
            self.module.on_text_frame(json.dumps(make_event(event_id="previous")))
            self.wait_for_status("logged", "previous")
            previous_history = json.loads(self.storage.values[STATE_FILE])["processed"]
            with patch.object(self.storage, "put", side_effect=put), self.assertLogs(self.module.__name__, "WARNING"):
                try:
                    self.module.on_text_frame(json.dumps(make_event(event_id="completion-retry")))
                    self.assertTrue(saved_retry.wait(timeout=2.0), "completion failure did not persist a retry")
                    document = json.loads(self.storage.values[STATE_FILE])
                    self.assertEqual(document["processed"], previous_history)
                    self.assertEqual(document["pending"][0]["event"]["event_id"], "completion-retry")
                    self.assertEqual(document["pending"][0]["attempts"], 1)
                    self.assertFalse(
                        any(
                            message["event_id"] == "completion-retry" and message["status"] == "logged"
                            for message in self.sender.messages
                        )
                    )
                finally:
                    release_retry.set()
                self.wait_for_status("logged", "completion-retry")
        self.assertTrue(failed_completion.is_set())
        document = json.loads(self.storage.values[STATE_FILE])
        self.assertEqual(document["pending"], [])
        self.assertEqual(document["processed"], ["test-producer::run-1::completion-retry"])

    def test_restoration_skips_malformed_records_without_losing_valid_work(self):
        """Recover valid queued work, run mappings and completion history beside malformed identities."""
        self.persist_pending(make_event())
        document = json.loads(self.storage.values[STATE_FILE])
        document["runs"] = {"test-producer::run-1": "saved-run"}
        document["processed"] = ["test-producer::run-1::previous"]
        document["pending"] = [
            {"event": {}},
            {"event": {"producer": "test-producer", "run_key": "run-1"}},
            {"event": {"producer": [], "run_key": "run-1", "event_id": "invalid"}},
            {"event": {"producer": "test-producer", "run_key": "run-1", "event_id": None}},
            {"event": None},
            None,
            *document["pending"],
        ]
        self.storage.put(STATE_FILE, json.dumps(document).encode())
        with self.assertLogs(self.module.__name__, level="WARNING") as captured:
            self.connect()
        self.assertEqual(len(captured.output), 6)
        self.wait_for_status("logged")
        self.assertEqual(self.client.metrics[0][0], "saved-run")
        self.assertEqual(self.client.created_runs, [])
        restored = json.loads(self.storage.values[STATE_FILE])
        self.assertEqual(restored["pending"], [])
        self.assertIn("test-producer::run-1::previous", restored["processed"])

    def test_event_is_persisted_before_queued_ack(self):
        """Persist an accepted event before acknowledging that it is queued."""
        self.connect()
        self.module.on_text_frame(json.dumps(make_event()))
        pending_put = timeline_index(
            self.timeline,
            lambda item: item[0] == "put" and json.loads(item[2])["pending"],
            "a state write containing the pending event",
        )
        queued_ack = timeline_index(
            self.timeline,
            lambda item: item[:2] == ("ack", "queued"),
            "the queued acknowledgement",
        )
        self.assertLess(pending_put, queued_ack)

    def test_connect_restores_and_processes_pending_event(self):
        """Replay a persisted event after a fresh bridge instance connects."""
        self.persist_pending(make_event())
        self.connect()
        wait_until(lambda: any(message["status"] == "logged" for message in self.sender.messages))
        self.assertEqual(len(self.client.metrics), 1)

    def test_duplicate_event_is_logged_once(self):
        """Log one metric when the same event identity is received twice."""
        self.connect()
        payload = json.dumps(make_event())
        self.module.on_text_frame(payload)
        self.module.on_text_frame(payload)
        wait_until(lambda: any(message["status"] == "logged" for message in self.sender.messages))
        time.sleep(0.05)
        self.assertEqual(len(self.client.metrics), 1)

    def test_invalid_retry_settings_do_not_stop_operational_recovery(self):
        """Fall back for malformed retry values and successfully process a later attempt."""
        settings = {
            "PYO3_MLFLOW_MAX_RETRIES": "abc",
            "PYO3_MLFLOW_RETRY_BASE_SECONDS": "abc",
            "PYO3_MLFLOW_RETRY_MAX_SECONDS": "1",
        }
        with (
            patch.dict(os.environ, settings),
            patch.object(self.client, "log_metric", side_effect=[OSError("temporary outage"), None]) as log_metric,
            self.assertLogs(self.module.__name__, level="WARNING"),
        ):
            self.connect()
            self.module.on_text_frame(json.dumps(make_event()))
            wait_until(lambda: any(message["status"] == "logged" for message in self.sender.messages))
        self.assertEqual(log_metric.call_count, 2)

    def test_duplicate_completed_event_is_acknowledged_without_logging_again(self):
        """Use persisted completion history when a producer retransmits an already logged event."""
        self.connect()
        payload = json.dumps(make_event())
        self.module.on_text_frame(payload)
        self.wait_for_status("logged")
        self.module.on_text_frame(payload)
        self.assertEqual(self.sender.messages[-1]["status"], "duplicate")
        self.assertEqual(len(self.client.metrics), 1)

    def test_full_outbox_rejects_new_work_but_retains_queued_duplicate(self):
        """Keep a deferred event intact when the durable admission limit is reached."""
        self.persist_pending(make_event(), next_attempt_at=time.time() + 3600)
        with patch.dict(os.environ, {"PYO3_MLFLOW_QUEUE_SIZE": "1"}):
            self.connect()
            self.module.on_text_frame(json.dumps(make_event()))
            self.assertEqual(self.sender.messages[-1]["status"], "queued")
            self.module.on_text_frame(json.dumps(make_event(event_id="overflow")))
            self.assertEqual(self.sender.messages[-1]["status"], "failed")
        document = json.loads(self.storage.values[STATE_FILE])
        self.assertEqual(len(document["pending"]), 1)
        self.assertEqual(document["pending"][0]["event"]["event_id"], "event-1")
        self.assertEqual(self.client.metrics, [])

    def test_corrupt_state_is_reported_and_new_events_can_still_be_logged(self):
        """Recover from unreadable or incompatible saved state with an explicit diagnostic."""
        for index, payload in enumerate((b"not json", b'{"schema_version":999}', b"\xff")):
            with self.subTest(payload=payload):
                self.module.on_shutdown()
                self.storage.values[STATE_FILE] = payload
                with self.assertLogs(self.module.__name__, level="ERROR"):
                    self.connect()
                event_id = f"recovered-{index}"
                self.module.on_text_frame(json.dumps(make_event(event_id=event_id)))
                self.wait_for_status("logged", event_id)
                self.assertEqual(json.loads(self.storage.values[STATE_FILE])["pending"], [])

    def test_startup_failures_retain_work_for_a_later_reconnect(self):
        """Keep persisted events when the SDK is missing or its client cannot initialise."""
        mlflow_module = sys.modules["mlflow"]
        failures = (
            patch.dict(sys.modules, {"mlflow": None}),
            patch.object(mlflow_module, "MlflowClient", side_effect=OSError("tracking configuration unavailable")),
        )
        for index, failure in enumerate(failures):
            with self.subTest(index=index):
                self.module.on_shutdown()
                event_id = f"startup-{index}"
                self.persist_pending(make_event(event_id=event_id))
                with failure, self.assertLogs(self.module.__name__, level="ERROR"):
                    self.connect()
                    self.module.on_shutdown()
                document = json.loads(self.storage.values[STATE_FILE])
                self.assertEqual(document["pending"][0]["event"]["event_id"], event_id)
                self.connect()
                self.wait_for_status("logged", event_id)
