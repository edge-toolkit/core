# Generic PyO3 MLflow bridge

`pyo3-mlflow` is an application-neutral Edge Toolkit tracking adapter. Producers
send `mlflow-event` JSON frames through the WebSocket hub; the bridge owns the
MLflow dependency, tracking URI, retry policy, durable outbox, and artifact
upload. Infrastructure logs and high-frequency system telemetry remain the
responsibility of OpenTelemetry/OpenObserve.

## Version 1 protocol

Modules send tracking data as JSON messages. Every version 1 message contains these fields:

- `type`: always `mlflow-event`;
- `schema_version`: always `1`;
- `producer`: a stable name for the module sending the event;
- `event`: the kind of tracking operation to perform;
- `run_key`: the producer's identifier for the logical MLflow run;
- `event_id`: an identifier for this event within the logical run.

Together, `producer`, `run_key`, and `event_id` uniquely identify an event. The bridge uses this identity to avoid
recording the same event more than once. Including the producer also prevents unrelated modules from colliding when
they use the same run or event identifiers.

The following example records a validation-loss measurement at step 3:

```json
{
  "type": "mlflow-event",
  "schema_version": 1,
  "producer": "my-edge-module",
  "event": "metrics",
  "event_id": "job-42-step-3",
  "run_key": "job-42",
  "step": 3,
  "metrics": { "validation_loss": 0.42 }
}
```

Each producer decides what `step` represents in its workflow. It could be an epoch, federated round, benchmark
iteration, or monitoring interval.

Supported event kinds are:

- `run-start`: optional `experiment`, `run_name`, `params`, and `tags`;
- `params`: add scalar parameters to an existing logical run;
- `tags`: add scalar tags to an existing logical run;
- `metrics`: log a non-empty numeric mapping at a non-negative `step`;
- `artifacts`: upload Edge Toolkit storage pointers;
- `run-end`: terminate a run as `FINISHED`, `FAILED`, or `KILLED`.

Artifacts use `{ "bucket", "filename", "artifact_path" }`, where `bucket` is the source storage agent ID.
Reads are denied unless the operator explicitly allows that bucket; the bridge's own bucket is always denied.
The allow-list applies to all producers using the bridge, not to a verified sender identity. Only buckets whose
contents may be shared with every participating producer should be allowed. Local filesystem paths are not accepted.

The bridge emits `mlflow-ack` frames with `queued`, `retrying`, `logged`,
`duplicate`, `rejected`, or `failed` status. The current PyO3 runner exposes
raw text/binary sending, so acknowledgements and events use the hub relay. A
future typed direct-message API can transport the same payload schema.

## Durability and restart behaviour

Pending events, the bounded processed-event history, and logical-run mappings
are persisted in the bridge's Edge Toolkit storage bucket. Events are replayed
after bridge restart. Processing remains ordered within each producer/run while
unrelated runs can progress independently. Before creating a run, the bridge
searches MLflow for the stable `edge_toolkit.run_identity` tag, preventing a
bridge restart from creating a duplicate run.

Operational failures use bounded retries. Unexpected event-processing exceptions are logged, the affected event is
removed from the pending queue with a `failed` acknowledgement, and unrelated events continue processing.

## Runtime integration

Run `@edge-toolkit/et-ws-pyo3-mlflow` with `et-ws-pyo3-runner`. The runner environment must provide the `mlflow`
Python package and network access to `MLFLOW_TRACKING_URI`. The bridge deliberately does not deploy an MLflow server;
scenario repositories choose the MLflow version, backing store, artifact store, and deployment topology they need.

## Configuration

- `MLFLOW_TRACKING_URI` defaults to `http://127.0.0.1:5000` and selects the MLflow API endpoint.
- `MLFLOW_DEFAULT_EXPERIMENT` defaults to `edge-toolkit` and applies when an event omits an experiment.
- `PYO3_MLFLOW_QUEUE_SIZE` defaults to `1000` and limits the durable pending-event queue.
- `PYO3_MLFLOW_MAX_RETRIES` defaults to `8` attempts before a terminal failure acknowledgement.
- `PYO3_MLFLOW_RETRY_BASE_SECONDS` defaults to `2` seconds for the initial exponential-backoff delay.
- `PYO3_MLFLOW_RETRY_MAX_SECONDS` defaults to `60` seconds and caps retry backoff.
- `PYO3_MLFLOW_DEDUP_HISTORY` defaults to `10000` persisted completed-event identities.
- `PYO3_MLFLOW_MAX_EVENT_BYTES` defaults to `262144` bytes per event frame.
- `PYO3_MLFLOW_MAX_ARTIFACT_BYTES` defaults to `52428800` bytes per artifact.
- `PYO3_MLFLOW_ARTIFACT_BUCKETS` is a comma-separated allow-list of source storage agent IDs; empty by default.
- `PYO3_MLFLOW_STATE_FILE` defaults to `pyo3-mlflow-bridge-state.json` and selects the state object name.

Malformed or non-positive integer settings use their documented defaults.

The producer namespace prevents accidental collisions; it is not authentication.
Untrusted deployments still require authenticated agents and authorization at
the WebSocket/storage boundary.
