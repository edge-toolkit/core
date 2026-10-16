# Recovery supervisor

`et-recovery-supervisor` is an opt-in Core service for observing application failures and coordinating bounded recovery.
It is separate from the WebSocket hub: the hub routes messages and serves storage, while the supervisor owns recovery
policy. Kubernetes performs deployment restarts; application modules decide how their work is checkpointed and resumed.
No machine-learning library, application adapter, dataset, or fault-injection tool is required by the service.

## Run

Create a recovery TOML configuration using the shape in `recovery.example.toml`. Supply the real hub address,
Kubernetes context, namespace, workload deployment names, agent identities, module sources, job ID, and state location.
The state directory must already exist and survive supervisor restarts. Start from the repository root:

```sh
mise run recovery-supervisor services/recovery-supervisor/recovery.example.toml
```

The example is illustrative: its Kubernetes context and workload are placeholders, not a generated deployment.
An installed binary accepts the same configuration path as its positional argument. `kubectl` must be on PATH with
credentials for the explicitly configured context. No deployment manifests are applied automatically.

The default mode is `observe-only`. Set `mode = "controller-assisted"` to permit restarts. Observe-only still writes
reports and its state file, but never requests deployment changes. A single supervisor owns each job and workload set.
A file lock prevents concurrent owners of the same state file; separate state files do not provide distributed leader
election, so operators must not run overlapping supervisors against the same deployments.

## Recovery policy

The supervisor combines deployment readiness with application lifecycle evidence received over the existing hub
agent-message transport. An unhealthy deployment, an explicit application failure, or active work that stops advancing
can open an incident. Modules that have not started work do not fail merely for being idle. Completed work does not
acquire another processing obligation, although an unhealthy deployment remains observable.

The detection grace period filters brief disruptions. Once failure is detected, the supervisor waits before requesting
a restart, allowing native Kubernetes recovery to act first. Restart requests have a cooldown and a per-workload budget
per job. Each request consumes its budget before execution, even if Kubernetes rejects it. The reservation
is persisted before the external command; a persistence failure stops the service without executing that command.
Serialization and state-file writes run off the async transport task. Saves remain ordered even when their caller is
cancelled, so a final shutdown report cannot be overwritten by an older in-flight save.
An unreadable or mismatched saved budget fails closed. Do not delete the state file to bypass an exhausted budget.

API errors are unknown observations, not evidence that an application failed. They cannot trigger restarts. Kubectl
processes have deadlines and are killed when cancelled; commands use explicit argument lists, not shell interpolation.
Supervised deployments must have exactly one desired replica. This initial backend does not choose replacement nodes,
change replica counts, move datasets, or repair a physical machine.

A Ready deployment is not sufficient to close a detected incident. Application progress must advance beyond its
pre-failure value, and the workload must no longer be unhealthy. Reporting a restored checkpoint alone does not prove
unfinished work continued. Missing evidence remains incomplete; crossing the recovery deadline stops further actions.
Later useful recovery is recorded with its actual duration, not presented as meeting the deadline.

## Application contract

Modules send a CloudEvent as the payload of an `et-send-agent-message` addressed to the configured supervisor agent.
This uses the existing direct-message path and acknowledgements without changing the hub wire protocol. A hub
`et-client-event` alone is logged by the hub, not delivered to the supervisor. The SDK supplies the envelope:

```python
from edge_toolkit.recovery import Checkpoints, Lifecycle

# Run after on_connect supplies the producing agent identity.
lifecycle = Lifecycle(send.text, "recovery-supervisor", "/modules/example-worker", "example-job")
lifecycle.emit("started", 0)
checkpoints = Checkpoints(storage, agent_id, "example-job-model", lifecycle=lifecycle)
restored = checkpoints.load()
# Reconstruct application state from restored bytes when a checkpoint exists.
# After committing one new unit of useful work:
checkpoints.save(b"application-defined state", 1)
lifecycle.emit("progress", 1)
# At the end of the job:
lifecycle.emit("completed", 1)
```

Every lifecycle payload has `schema_version = 1`, `job_id`, `instance`, `sequence`, and unsigned `progress` fields.
The SDK creates a new process instance and increasing sequence numbers. Event types are `et.recovery.started`,
`et.recovery.progress`, `et.recovery.failed`, `et.recovery.completed`, `et.recovery.checkpoint-saved`,
`et.recovery.checkpoint-restored`, and `et.recovery.resume-completed`. Checkpoint events also carry `bytes_count` and
`duration_millis`. The application can emit `resume-completed` after reconstructing its in-memory state; resuming useful
processing is verified separately through progress. Progress units are application-defined committed work, not attempted
operations: training rounds, completed batches, or completed inference tasks, for example.

The exact configured sender, module source, and job must match. The first observed instance can be adopted mid-run,
with its existing counter establishing a baseline rather than proving recovery. Replacement instances announce
`started` first; replayed sequence numbers and retired instances are rejected within the observer session. These
bindings reduce accidental cross-talk, but hub agent IDs and CloudEvent sources are not authenticated identities. Use
controller-assisted mode only on a trusted hub/network. This is not malicious-node detection or a security boundary
against another hub participant claiming an allowlisted agent ID. Kubernetes credentials belong to the supervisor,
never to browser modules.

Checkpoint payloads are immutable objects followed by an integrity-checked latest pointer in the producing agent
namespace. A failed pointer commit leaves the previous checkpoint available. Loading rejects corrupt, missing,
oversized, or foreign-namespace pointers/payloads. The application owns serialization, safe commit boundaries, optimizer
state, participation rules, and duplicate-work handling; never deserialize untrusted executable formats. Use one writer
per checkpoint key. Unreferenced payloads are not garbage-collected automatically. Integrity hashes do not authenticate
producers. Guarantees depend on whole-object pointer writes and storage surviving worker restarts; hub/storage
loss and power-loss durability are not solved here. Telemetry delivery failures are logged without undoing a successful
checkpoint operation.

## Evidence and measurements

The state file contains the durable restart budget and latest JSON report. Reports distinguish suspected/transient,
detected, recovered, and unrecovered incidents, action requests and failures, unknown infrastructure observations, and
checkpoint save/load/resume measurements. `failure_detection_seconds` measures grace-qualified detection from first
unhealthy observation. `recovery_seconds` measures from that observation to verified useful progress. `mttr_seconds`
averages recovered incidents only and is null when none recovered; incomplete incidents remain in the report. These are
observer-relative measurements, not injection-to-recovery times or continuous uptime guarantees. Polling and command
latency limit resolution. Checkpoint timing is producer-reported; cross-node clock synchronisation is not needed for
those durations. Resume efficiency requires application-specific work-attempt evidence and is not inferred here.

Incident detection and recovery timestamps are null until observed, rather than using zero as a missing value.
Action records contain a `kind` (`restart-reserved`, `restart-requested`, or `restart-failed`); failed actions also
contain an `error` description. Checkpoint records retain their operation kind, instance, progress, size and duration.

Budget counters survive supervisor restarts. Lifecycle sequence tracking and incident histories belong to one observer
session and are not restored; archive reports before restarting if historical measurements are needed. A lost hub
connection stops the service and its actions rather than continuing to act on stale application evidence. Restart the
service using the same state file after connectivity returns; this first version does not reconnect its live session.

## Permissions and verification

Restrict Kubernetes access to `get` and `patch` on the named Deployments in the configured namespace. `rollout restart`
uses a patch; no Pod deletion, secret reads, cluster-wide access, or arbitrary command execution is required. Operators
must provide appropriately scoped credentials. Run the supervisor outside the supervised workloads, with persistent
state storage. Automatic scenario-generator wiring and service container packaging are not included in this increment.

```sh
mise run test-recovery-supervisor
MISE_ENV=python mise run test-edge-toolkit
```

The tests use deterministic policy observations, a fake deployment backend, and real in-process hub transport. They do
not inject faults or mutate a live cluster. Fault injection, workload-specific benchmark scenarios, repetition
statistics and experimental SLO evaluation remain independent consumers of Core recovery evidence.
