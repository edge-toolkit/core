# Help for `et-recovery-supervisor`

`et-recovery-supervisor` takes no command-line arguments; it reads its configuration from the environment.

## Environment variables

### `RECOVERY_AGENT_ID`

Type: string. Required.

Stable identity reserved for this supervisor.

### `RECOVERY_COMMAND_TIMEOUT_SECONDS`

Type: integer. Required.

Bound on each kubectl process, including credential-plugin execution.

### `RECOVERY_CONTEXT`

Type: string. Required.

Explicit Kubernetes context; never relies on the operator's current context.

### `RECOVERY_JOB_ID`

Type: string. Required.

Application job whose lifecycle events are eligible.

### `RECOVERY_MODE`

Type: one of `observe-only`, `controller-assisted`. Default: `observe-only`.

Observe-only unless explicitly enabled.

### `RECOVERY_NAMESPACE`

Type: string. Required.

Namespace containing every allowlisted deployment.

### `RECOVERY_POLICY_DEADLINE_SECONDS`

Type: integer. Required.

Maximum time from first unhealthy observation to useful recovery.

### `RECOVERY_POLICY_DETECTION_GRACE_SECONDS`

Type: integer. Required.

Continuous unhealthy observation before declaring a failure.

### `RECOVERY_POLICY_MAX_RESTARTS`

Type: integer. Required.

Lifetime restart request budget per configured workload and job, persisted before executing an action.

### `RECOVERY_POLICY_RESTART_AFTER_SECONDS`

Type: integer. Required.

Minimum time between detection and an action, and between subsequent attempts.

### `RECOVERY_POLICY_STALL_SECONDS`

Type: integer. Required.

Active work without advancing progress is considered stalled after this interval.

### `RECOVERY_POLL_SECONDS`

Type: integer. Required.

Time between infrastructure observation passes.

### `RECOVERY_STATE_FILE`

Type: string. Required.

Durable restart budget and latest recovery report; parent directory must exist.

### `RECOVERY_WORKLOADS`

Type: string. Required.

Explicit restart allowlist and sender bindings.

### `RECOVERY_WS_URL`

Type: string. Required.

Hub WebSocket URL.
