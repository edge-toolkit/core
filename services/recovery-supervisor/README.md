# Recovery supervisor

`et-recovery-supervisor` is an opt-in Core service for observing application failures and coordinating bounded recovery.
It is separate from the WebSocket hub: the hub routes messages and serves storage, while the supervisor owns recovery
policy. Kubernetes performs deployment restarts; application modules decide how their work is checkpointed and resumed.
No machine-learning library, application adapter, dataset, or fault-injection tool is required by the service.

## Scenario deployment

Add an optional `recovery:` block to a k3s scenario. Omit it for an ordinary scenario without a supervisor:

```yaml
cluster_name: example
deployment_type: k3s
agents:
  - name: example-worker
    runner: pyo3
    resources:
      - type: pyo3-math1
recovery:
  job_id: example-job
  mode: observe-only
  poll_seconds: 2
  command_timeout_seconds: 10
  policy:
    detection_grace_seconds: 3
    restart_after_seconds: 20
    stall_seconds: 180
    deadline_seconds: 240
    max_restarts: 2
  workloads:
    - deployment: example-worker
      agent_id: example-worker
      source: /modules/@edge-toolkit/et-ws-pyo3-math1
```

Generate and deploy the scenario using the normal CLI workflow. The output includes the supervisor Deployment,
`RECOVERY_*` settings, ServiceAccount, named-Deployment Role and RoleBinding, projected-token kubeconfig and persistent
state claim. Observe-only grants `get`; controller-assisted also grants `patch`. No host kubeconfig is mounted.
The generated README includes image preparation and deployment commands. Recovery currently requires k3s output;
other formats are rejected rather than silently omitting supervision. Optional `recovery.limits` uses the standard
scenario resource-limit fields.

Supervised targets must be declared, continuously running native-Python workloads. Their generated settings include
`PYO3_AGENT_ID`, `RECOVERY_JOB_ID` and `RECOVERY_SUPERVISOR_AGENT_ID`. The module must use the shared job/supervisor
settings, emit matching lifecycle evidence and implement its own restoration logic. The math1 example above illustrates
deployment wiring only: math1 does not itself report recovery progress or restore checkpoints. Generating its supervisor
does not add these application behaviors automatically. Scheduled and other runner types are not supported targets.
Application dependencies remain the module author's responsibility. Modules importing Core's Python recovery helpers
need the SDK installed in their runner image; scenario generation does not install Python packages.

Supervisor and supervised Deployments use `Recreate` replacement. The supervisor has one replica and persistent budget
state; it is not a highly available controller. A different job cannot reuse an old job's state: archive the old report
and deliberately provision fresh state before changing the job. Do not run a second recovery controller over the same
workloads. Hub or physical-node loss is not covered by this deployment integration.

## Supervisor health

The image includes `et-recovery-healthcheck`, which reads the same `RECOVERY_*` settings as the service. It performs
local checks only: no network listener, Kubernetes command, or recovery action is involved. Success requires a held
budget lease and a recent completed supervision cycle from its current job and session. Startup, stale or malformed
evidence, old sessions, and an exited supervisor fail the probe. A sibling `<state-stem>.health.json` file records the
heartbeat only after observations, policy actions and report persistence finish; shutdown writes do not refresh it.
The `<state-stem>.lock` file holds exclusive ownership only; readable session identity lives in
`<state-stem>.session`, written after the lease is acquired. Keeping these separate lets health probes work with
Windows mandatory file locks. A leftover identity or heartbeat without a live lease never establishes health.

The freshness allowance includes the poll interval, bounded observation and potential restart commands for every
configured workload, and ten seconds of I/O slack. Future timestamps fail closed; this local freshness test does not
change the monotonic clocks used for recovery metrics. A supervised workload failure or an unavailable Kubernetes
observation does not itself make the supervisor unhealthy while its loop continues advancing. Unknown observations
remain visible in its recovery report rather than being presented as healthy applications.

Docker runs the probe every ten seconds, with a sixty-second startup grace and three attempts before reporting an
unhealthy container. Adjust the container's startup grace for unusually large workload sets. Generated k3s deployments
use the same executable for startup, readiness and liveness. Their startup budget includes registration and a bounded
cycle; after startup, three failed liveness probes can restart the supervisor. Kubernetes does not inherit Docker's
health check. Such a restart preserves the durable action budget but begins a new observer session, not a continuation
of its incident history.

## Standalone example

The binary reads environment settings through serde-env and takes no configuration-file argument. Supply real identities
and a surviving state directory, then start it separately from the application:

```sh
export RECOVERY_AGENT_ID=recovery-supervisor
export RECOVERY_WS_URL=ws://127.0.0.1:8080/ws
export RECOVERY_CONTEXT=k3d-example
export RECOVERY_NAMESPACE=example
export RECOVERY_JOB_ID=example-job
export RECOVERY_MODE=observe-only
export RECOVERY_STATE_FILE=target/recovery-state.json
export RECOVERY_POLL_SECONDS=2
export RECOVERY_COMMAND_TIMEOUT_SECONDS=10
export RECOVERY_POLICY_DETECTION_GRACE_SECONDS=3
export RECOVERY_POLICY_RESTART_AFTER_SECONDS=20
export RECOVERY_POLICY_STALL_SECONDS=180
export RECOVERY_POLICY_DEADLINE_SECONDS=240
export RECOVERY_POLICY_MAX_RESTARTS=2
export RECOVERY_WORKLOADS='[
  {"deployment":"example-worker","agent_id":"example-worker","source":"/modules/example-worker"}
]'
mise run recovery-supervisor
```

`kubectl` must be on PATH with credentials for the explicitly configured context. Standalone startup does not apply
deployment manifests. The state directory must already exist and survive supervisor restarts.

`RECOVERY_WORKLOADS` is a JSON array of objects with `deployment`, `agent_id`, and `source` fields. It remains an array
for a single workload. The loader explicitly decodes this JSON; malformed entries and unknown workload fields fail
startup. Policy settings use `RECOVERY_POLICY_*`, such as `RECOVERY_POLICY_STALL_SECONDS`. Runtime targets and policy
are validated before observing or changing deployments.

The default is `observe-only`. Set `RECOVERY_MODE=controller-assisted` to permit restarts. Observe-only still writes
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
On Unix, successful state replacement also syncs the parent directory so the renamed budget survives a filesystem
crash. Windows retains atomic replacement and file syncing; it does not claim equivalent directory-sync durability.

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

Use `wss` across untrusted networks. The supervisor validates TLS certificates using the operating system's trusted
roots; a self-signed server certificate must be trusted there too, not just accepted in a browser. Plain `ws` remains
supported for loopback and trusted internal deployments. URL validation does not turn an unencrypted connection into a
secure one. When the bounded evidence queue fills, delivery waits for capacity before transport acknowledgement
instead of discarding lifecycle events. Outbound keepalive pings continue while application data reads are
backpressured. Acknowledgement confirms queue admission, not successful policy processing or durable persistence.
Observation cycles drain a bounded batch so sustained producers cannot indefinitely postpone health reporting.
Keep reporting rates below the observer's capacity; backpressure does not increase processing throughput.

Checkpoints alternate between two bounded payload slots and an integrity-checked latest pointer in the producing agent
namespace. The inactive slot is written before the pointer changes, so a failed commit leaves the previous checkpoint
available. Repeated or failed saves create at most two payloads and one pointer per key. Loading rejects corrupt,
missing, oversized, or foreign-namespace pointers/payloads. The application owns serialization, safe commit boundaries,
optimizer state, participation rules and duplicate-work handling. Never deserialize untrusted executable formats. Use
one writer per checkpoint key and one shared helper for concurrent access within a process. Legacy content-addressed
checkpoints remain readable; future saves use the bounded slots, but orphaned legacy objects require operator cleanup.
Saving requires the checkpoint namespace to match `storage.agent_id`, the runner's current registered identity.
After an identity change, an old helper may still load the previous agent's checkpoint, but saves are rejected before
storage access. Create a helper bound to the new identity before saving resumed work in its namespace.
Integrity hashes do not authenticate producers. Guarantees depend on whole-object pointer writes and storage surviving
worker restarts; hub/storage loss and power-loss durability are not solved here. Telemetry delivery failures are logged
without undoing a successful checkpoint operation.

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
state storage. Generated deployments provide these permissions and persistent storage automatically.

```sh
mise run test-recovery-supervisor
MISE_ENV=python mise run test-edge-toolkit
```

The tests use deterministic policy observations, a fake deployment backend, and real in-process hub transport. They do
not inject faults or mutate a live cluster. Fault injection, workload-specific benchmark scenarios, repetition
statistics and experimental SLO evaluation remain independent consumers of Core recovery evidence.
