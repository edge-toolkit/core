"""Black-box tests for MLflow bridge lifecycle."""

import json
import os
import sys
import types
import unittest
from unittest.mock import patch

from .support import STATE_FILE, BridgeFixtureMixin, UnexpectedClientError, make_event, wait_until


class LifecycleTests(BridgeFixtureMixin, unittest.TestCase):
    """Exercise bridge lifecycle through its public lifecycle hooks."""

    def test_advertised_sdk_exception_uses_operational_failure_handling(self):
        """Recognise the SDK's declared exception type instead of classifying it as an unexpected bug."""
        sdk_exceptions = types.SimpleNamespace(MlflowException=UnexpectedClientError)
        with (
            patch.object(sys.modules["mlflow"], "exceptions", sdk_exceptions, create=True),
            patch.dict(os.environ, {"PYO3_MLFLOW_MAX_RETRIES": "1"}),
            patch.object(self.client, "log_metric", side_effect=UnexpectedClientError("SDK request failed")),
            self.assertLogs(self.module.__name__, level="ERROR") as captured,
        ):
            self.connect()
            self.module.on_text_frame(json.dumps(make_event()))
            self.wait_for_status("failed")
        failure = self.sender.messages[-1]
        self.assertIn("retry limit reached: SDK request failed", failure["message"])
        self.assertIn("exhausted 1 attempts", "\n".join(captured.output))
        self.assertNotIn("unexpected failure", "\n".join(captured.output))
        self.module.on_text_frame(json.dumps(make_event(event_id="healthy-after-sdk-error")))
        self.wait_for_status("logged", "healthy-after-sdk-error")
        self.assertEqual(len(self.client.metrics), 1)

    def test_missing_storage_handle_allows_in_memory_processing(self):
        """Exercise the no-storage guards without claiming that tracking events have been persisted."""
        self.module.init(self.sender, None)
        self.connect()
        self.module.on_text_frame(json.dumps(make_event()))
        self.wait_for_status("logged")
        self.assertEqual(len(self.client.metrics), 1)
        self.assertEqual(self.storage.values, {})
        self.assertFalse(any(action[0] in {"get", "put"} for action in self.timeline))

    def test_empty_registration_identity_does_not_access_agent_storage(self):
        """Avoid storage reads and writes when a registration hook supplies no usable agent identity."""
        self.module.on_connect("")
        self.module.on_text_frame(json.dumps(make_event()))
        self.wait_for_status("logged")
        self.assertEqual(len(self.client.metrics), 1)
        self.assertFalse(any(action[0] in {"get", "put"} for action in self.timeline))

    def test_missing_sender_does_not_prevent_tracking_or_completion(self):
        """Process and persist an event even when no acknowledgement transport was provided."""
        self.module.init(None, self.storage)
        self.connect()
        self.module.on_text_frame(json.dumps(make_event()))
        wait_until(
            lambda: STATE_FILE in self.storage.values and bool(json.loads(self.storage.values[STATE_FILE])["processed"])
        )
        self.assertEqual(len(self.client.metrics), 1)
        self.assertEqual(self.sender.messages, [])
        self.assertEqual(json.loads(self.storage.values[STATE_FILE])["pending"], [])

    def test_producer_namespaces_create_independent_runs(self):
        """Create independent MLflow runs for otherwise identical producer events."""
        self.connect()
        self.module.on_text_frame(json.dumps(make_event(producer="one")))
        self.module.on_text_frame(json.dumps(make_event(producer="two")))
        wait_until(lambda: len(self.client.metrics) == 2)
        producers = {tags["edge_toolkit.producer"] for _, tags in self.client.created_runs}
        self.assertEqual(producers, {"one", "two"})

    def test_unexpected_client_errors_do_not_stop_other_events(self):
        """Fail poison events explicitly and keep draining healthy events without reconnecting."""
        self.connect()
        for index, error in enumerate((KeyError("missing SDK field"), UnexpectedClientError("SDK failure"))):
            with self.subTest(error=type(error).__name__):
                event_id = f"poison-{index}"
                with (
                    patch.object(self.client, "log_metric", side_effect=error),
                    self.assertLogs(self.module.__name__, level="ERROR"),
                ):
                    self.module.on_text_frame(json.dumps(make_event(event_id=event_id)))
                    wait_until(
                        lambda event_id=event_id: any(
                            message["event_id"] == event_id and message["status"] == "failed"
                            for message in self.sender.messages
                        )
                    )
                healthy_id = f"healthy-{index}"
                self.module.on_text_frame(json.dumps(make_event(event_id=healthy_id)))
                wait_until(
                    lambda healthy_id=healthy_id: any(
                        message["event_id"] == healthy_id and message["status"] == "logged"
                        for message in self.sender.messages
                    )
                )
        document = json.loads(self.storage.values[STATE_FILE])
        self.assertEqual(document["pending"], [])
        self.assertEqual(len(self.client.metrics), 2)

    def test_run_lifecycle_logs_parameters_tags_and_final_metrics(self):
        """Dispatch each supported non-artifact operation in order within one logical run."""
        self.connect()
        events = [
            make_event(
                event_id="start", event="run-start", experiment="demo", run_name="Example", params={"epochs": 3}
            ),
            make_event(event_id="params", event="params", params={"enabled": True}),
            make_event(event_id="tags", event="tags", tags={"dataset": "held-out"}),
            make_event(event_id="end", event="run-end", metrics={"test_loss": 0.25}, step=4),
        ]
        for event in events:
            self.module.on_text_frame(json.dumps(event))
        self.wait_for_status("logged", "end")
        self.assertEqual(self.client.params, [("run-1", "epochs", "3"), ("run-1", "enabled", "True")])
        self.assertEqual(self.client.tags, [("run-1", "dataset", "held-out")])
        self.assertEqual(self.client.metrics[0][1:3], ("test_loss", 0.25))
        self.assertEqual(self.client.metrics[0][-1], 4)
        self.assertEqual(self.client.terminated, [("run-1", "FINISHED")])
        self.assertEqual(self.client.created_runs[0][1]["mlflow.runName"], "Example")

    def test_run_can_end_without_final_metrics(self):
        """Terminate a run with an explicit status without inventing a final metric."""
        self.connect()
        event = make_event(event="run-end", status="KILLED")
        event.pop("metrics")
        self.module.on_text_frame(json.dumps(event))
        self.wait_for_status("logged")
        self.assertEqual(self.client.terminated, [("run-1", "KILLED")])
        self.assertEqual(self.client.metrics, [])

    def test_existing_remote_run_is_reused_when_experiment_is_created(self):
        """Use a remotely discovered run rather than creating a duplicate after losing local mappings."""
        found = types.SimpleNamespace(info=types.SimpleNamespace(run_id="recovered-run"))
        with (
            patch.object(self.client, "get_experiment_by_name", return_value=None),
            patch.object(self.client, "create_experiment", return_value="new-experiment") as create_experiment,
            patch.object(self.client, "search_runs", return_value=[found]) as search_runs,
        ):
            self.connect()
            self.module.on_text_frame(json.dumps(make_event(experiment="new-experiment")))
            self.wait_for_status("logged")
        create_experiment.assert_called_once_with("new-experiment")
        self.assertEqual(search_runs.call_args.kwargs["experiment_ids"], ["new-experiment"])
        self.assertIn("test-producer::run-1", search_runs.call_args.kwargs["filter_string"])
        self.assertEqual(self.client.created_runs, [])
        self.assertEqual(self.client.metrics[0][0], "recovered-run")

    def test_reconnect_does_not_start_a_second_worker_or_reload_state(self):
        """Reuse a live worker on reconnect without replaying storage a second time."""
        self.connect()
        self.connect()
        self.module.on_text_frame(json.dumps(make_event()))
        self.wait_for_status("logged")
        self.assertEqual(self.timeline.count(("get", "mlflow-agent", STATE_FILE)), 1)
        self.assertEqual(len(self.client.metrics), 1)
