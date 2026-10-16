//! Deterministic recovery decisions driven by observer time and application evidence, not by benchmark actions.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use edge_toolkit::ws::CloudEvent;
use serde::{Deserialize, Serialize};

use crate::RecoveryError;
use crate::config::{Config, Mode, Workload};

/// Infrastructure observations distinguish API failure from an unhealthy deployment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Health {
    /// The desired single replica is available at its current generation.
    Ready,
    /// The deployment exists but is not ready, or has no running replica.
    Unready,
    /// Observation failed; no new recovery action is safe on that evidence alone.
    Unknown,
}

/// Decision emitted by a policy step; only the trusted runtime executes restarts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Decision {
    /// Observe without modifying the deployment.
    Observe,
    /// A restart request is eligible, subject to durable budget reservation.
    Restart,
}

/// An incident's measurable lifecycle; incomplete incidents are never counted as successful recoveries.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// Unhealthy evidence has not yet satisfied the detection grace period.
    Suspected,
    /// Failure detected; useful recovery has not been verified.
    Detected,
    /// Application useful progress advanced beyond its pre-failure value.
    Recovered,
    /// Recovery deadline passed without useful progress.
    Unrecovered,
    /// Unhealthy observation cleared before the detection grace period elapsed.
    Transient,
}

/// A missing observation is distinct from a real timestamp at observer second zero.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(untagged)]
pub enum ObservationTime {
    /// This transition has not been observed; serialized as null.
    Pending,
    /// Elapsed observer seconds when the transition occurred.
    Recorded(u64),
}

/// A recovery action's outcome is separate from evidence that useful work resumed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Action {
    /// Budget was reserved before attempting an external operation.
    RestartReserved,
    /// Kubernetes accepted the restart request.
    RestartRequested,
    /// The backend rejected or could not deliver the restart request.
    RestartFailed {
        /// Operational error returned by the backend.
        error: String,
    },
}

/// Checkpoint operations reported by an application, not inferred from container readiness.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum CheckpointKind {
    /// A checkpoint was committed to storage.
    #[serde(rename = "checkpoint-saved")]
    Saved,
    /// Saved bytes were loaded and validated.
    #[serde(rename = "checkpoint-restored")]
    Restored,
    /// The application reconstructed its in-memory state.
    #[serde(rename = "resume-completed")]
    ResumeCompleted,
}

/// One validated application checkpoint measurement.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct CheckpointMeasurement {
    /// Deployment reporting the operation.
    pub workload: String,
    /// Checkpoint operation performed.
    pub kind: CheckpointKind,
    /// Application process instance.
    pub instance: String,
    /// Observer-relative receipt time in seconds.
    pub observed_at: u64,
    /// Application-defined completed work counter.
    pub progress: u64,
    /// Payload size in bytes.
    pub bytes: u64,
    /// Producer-measured operation duration.
    pub duration_millis: u64,
}

/// An unavailable measurement must not be interpreted as a zero-duration success.
#[derive(Clone, Copy, Debug, Serialize)]
#[non_exhaustive]
#[serde(untagged)]
pub enum MeanSeconds {
    /// No eligible observations have been recorded; serialized as null.
    Unavailable,
    /// Arithmetic mean of eligible durations, in seconds.
    Measured(f64),
}

/// Owned report snapshot, safe to serialize off the async control task.
#[derive(Clone, Debug, Serialize)]
#[non_exhaustive]
pub struct Report {
    /// Persisted report format version.
    pub schema_version: u32,
    /// Job supervised by this observer.
    pub job_id: String,
    /// Whether external recovery actions are permitted.
    pub mode: Mode,
    /// Time coordinate used for incident observations.
    pub clock_basis: &'static str,
    /// Incidents recorded during this observer session.
    pub incidents: Vec<Incident>,
    /// Failed infrastructure observations, distinct from workload failures.
    pub unknown_observations: u64,
    /// Durations from first unhealthy observation to verified useful recovery.
    pub recovery_seconds: Vec<u64>,
    /// Number of incidents with verified recovery.
    pub recovered_incidents: usize,
    /// Durable job-level recovery action counters.
    pub restart_budget: Budget,
    /// Durations from first unhealthy observation to grace-qualified detection.
    pub failure_detection_seconds: Vec<u64>,
    /// Mean detection duration, or null without eligible observations.
    pub mean_failure_detection_seconds: MeanSeconds,
    /// Mean useful recovery duration, or null without verified recoveries.
    pub mttr_seconds: MeanSeconds,
    /// Number of grace-qualified failures, including those not recovered.
    pub detected_incidents: usize,
    /// Application-reported checkpoint operations.
    pub checkpoint_measurements: Vec<CheckpointMeasurement>,
}

/// Incident times are elapsed observer seconds within one supervisor process, not producer timestamps.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct Incident {
    /// Deployment affected by the incident.
    pub workload: String,
    /// First unhealthy observation.
    pub onset: u64,
    /// Grace-qualified detection time, or pending before detection.
    pub detected: ObservationTime,
    /// Verified useful recovery time, or pending before recovery.
    pub recovered: ObservationTime,
    /// Useful work completed before the incident.
    pub baseline: u64,
    /// Incident outcome.
    pub status: Status,
    /// Executed action outcomes, distinct from Kubernetes' own process recovery.
    pub actions: Vec<Action>,
}

#[derive(Debug, Default)]
enum Phase {
    #[default]
    Healthy,
    Incident(usize),
}

#[derive(Debug, Default)]
struct State {
    active: bool,
    failed: bool,
    completed: bool,
    progress: u64,
    progress_at: u64,
    instance: String,
    retired: BTreeSet<String>,
    sequence: u64,
    phase: Phase,
    last_action: u64,
}

/// Durable action budget. An attempted restart consumes budget even if the backend fails or the process exits.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
#[serde(deny_unknown_fields)]
pub struct Budget {
    /// Persisted format version.
    pub schema_version: u32,
    /// Job identity prevents accidentally reusing another job's budget.
    pub job_id: String,
    /// Lifetime restart requests per deployment.
    pub restarts: BTreeMap<String, u32>,
}

impl Budget {
    /// Begin a new job with no recovery actions consumed.
    #[must_use]
    pub const fn new(job_id: String) -> Self {
        Self {
            schema_version: 1,
            job_id,
            restarts: BTreeMap::new(),
        }
    }
}

/// A policy instance supervises exactly one configured job; no application event can change its target allowlist.
#[derive(Debug)]
pub struct Supervisor {
    config: Config,
    states: BTreeMap<String, State>,
    /// Measured incidents for this observer process.
    incidents: Vec<Incident>,
    /// Durable lifetime action counters.
    budget: Budget,
    /// API failures counted separately from workload failures.
    unknown_observations: u64,
    checkpoints: Vec<CheckpointMeasurement>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EventKind {
    Started,
    Progress,
    Failed,
    Completed,
    Checkpoint(CheckpointKind),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Evidence {
    schema_version: u32,
    job_id: String,
    instance: String,
    sequence: u64,
    progress: u64,
    #[serde(default)]
    bytes_count: u64,
    #[serde(default)]
    duration_millis: u64,
}

impl Supervisor {
    /// Incidents observed in this process, including incomplete and transient observations.
    #[must_use]
    pub fn incidents(&self) -> &[Incident] {
        &self.incidents
    }

    /// Durable restart counts; callers persist this budget before executing a reserved action.
    #[must_use]
    pub const fn budget(&self) -> &Budget {
        &self.budget
    }

    /// Construct policy state after validating both configuration and any restored action budget.
    pub fn new(config: Config, budget: Budget) -> Result<Self, RecoveryError> {
        config.validate()?;
        if budget.schema_version != 1 || budget.job_id != config.job_id {
            return Err(RecoveryError::Invalid(
                "restart budget version or job identity does not match configuration".into(),
            ));
        }
        let states = config
            .workloads
            .iter()
            .map(|item| (item.deployment.clone(), State::default()))
            .collect();
        Ok(Self {
            config,
            states,
            incidents: Vec::new(),
            budget,
            unknown_observations: 0,
            checkpoints: Vec::new(),
        })
    }

    /// Consume an allowlisted sender's lifecycle `CloudEvent`, rejecting invalid, replayed, or foreign evidence.
    pub fn event(&mut self, sender: &str, event: &CloudEvent, now: u64) -> Result<(), RecoveryError> {
        let workload = self
            .config
            .workloads
            .iter()
            .find(|item| item.agent_id == sender && item.source == event.source)
            .ok_or_else(|| RecoveryError::Invalid("lifecycle sender/source is not allowlisted".into()))?;
        let (kind, evidence) = event_evidence(event, &self.config.job_id)?;
        let state = self
            .states
            .get_mut(&workload.deployment)
            .ok_or_else(|| RecoveryError::Invalid("workload state is absent".into()))?;
        let first_instance = state.instance.is_empty();
        if state.retired.contains(&evidence.instance)
            || (evidence.instance == state.instance && evidence.sequence <= state.sequence)
            || (!first_instance && evidence.instance != state.instance && kind != EventKind::Started)
        {
            return Err(RecoveryError::Invalid(
                "stale evidence or instance must announce started first".into(),
            ));
        }
        if evidence.instance != state.instance {
            if !state.instance.is_empty() {
                let _inserted = state.retired.insert(state.instance.clone());
            }
            state.instance = evidence.instance;
        }
        state.sequence = evidence.sequence;
        if first_instance {
            state.progress = evidence.progress;
            if let Phase::Incident(index) = state.phase {
                self.incidents
                    .get_mut(index)
                    .ok_or_else(|| RecoveryError::Invalid("incident state is absent".into()))?
                    .baseline = evidence.progress;
            }
        }
        let advances_work = matches!(kind, EventKind::Progress | EventKind::Completed);
        if !state.active || (advances_work && evidence.progress > state.progress) {
            state.progress_at = now;
        }
        state.active = true;
        if !matches!(kind, EventKind::Checkpoint(_)) {
            state.failed = kind == EventKind::Failed;
            state.completed = kind == EventKind::Completed;
        }
        if advances_work {
            state.progress = state.progress.max(evidence.progress);
        }
        if let EventKind::Checkpoint(kind) = kind {
            self.checkpoints.push(CheckpointMeasurement {
                workload: workload.deployment.clone(),
                kind,
                instance: state.instance.clone(),
                observed_at: now,
                progress: evidence.progress,
                bytes: evidence.bytes_count,
                duration_millis: evidence.duration_millis,
            });
        }
        Ok(())
    }

    /// Observe one workload at monotonic elapsed seconds. Readiness alone never closes a detected incident.
    pub fn observe(&mut self, workload: &Workload, health: Health, now: u64) -> Result<Decision, RecoveryError> {
        let state = self
            .states
            .get_mut(&workload.deployment)
            .ok_or_else(|| RecoveryError::Invalid("workload is not configured".into()))?;
        if health == Health::Unknown {
            self.unknown_observations = self.unknown_observations.saturating_add(1);
            return Ok(Decision::Observe);
        }
        // An idle module is not stalled, and completing a job does not create another work obligation.
        let unhealthy = health == Health::Unready
            || state.failed
            || (state.active
                && !state.completed
                && now.saturating_sub(state.progress_at) >= self.config.policy.stall_seconds);
        if matches!(state.phase, Phase::Healthy) && unhealthy {
            let index = self.incidents.len();
            self.incidents.push(Incident {
                workload: workload.deployment.clone(),
                onset: now,
                detected: ObservationTime::Pending,
                recovered: ObservationTime::Pending,
                baseline: state.progress,
                status: Status::Suspected,
                actions: Vec::new(),
            });
            state.phase = Phase::Incident(index);
        }
        let Phase::Incident(index) = state.phase else {
            return Ok(Decision::Observe);
        };
        let incident = self
            .incidents
            .get_mut(index)
            .ok_or_else(|| RecoveryError::Invalid("incident state is absent".into()))?;
        state.advance(incident, health, unhealthy, &self.config.policy, now);
        let attempts = self
            .budget
            .restarts
            .get(&workload.deployment)
            .copied()
            .unwrap_or_default();
        if incident.status == Status::Detected
            && self.config.mode == Mode::ControllerAssisted
            && attempts < self.config.policy.max_restarts
            && now.saturating_sub(state.last_action) >= self.config.policy.restart_after_seconds
        {
            return Ok(Decision::Restart);
        }
        Ok(Decision::Observe)
    }

    /// Reserve an action before persisting the budget and executing the backend operation.
    pub fn reserve_restart(&mut self, workload: &Workload, now: u64) -> Result<(), RecoveryError> {
        let state = self
            .states
            .get_mut(&workload.deployment)
            .ok_or_else(|| RecoveryError::Invalid("restart target is not configured".into()))?;
        let Phase::Incident(index) = state.phase else {
            return Err(RecoveryError::Invalid("no incident permits this restart".into()));
        };
        let count = self.budget.restarts.entry(workload.deployment.clone()).or_default();
        let incident = self
            .incidents
            .get_mut(index)
            .ok_or_else(|| RecoveryError::Invalid("incident state is absent".into()))?;
        if self.config.mode != Mode::ControllerAssisted
            || *count >= self.config.policy.max_restarts
            || now.saturating_sub(state.last_action) < self.config.policy.restart_after_seconds
            || incident.status != Status::Detected
        {
            return Err(RecoveryError::Invalid(
                "restart is not eligible under the configured policy".into(),
            ));
        }
        *count = count.saturating_add(1);
        state.last_action = now;
        incident.actions.push(Action::RestartReserved);
        Ok(())
    }

    /// Record whether a reserved backend request succeeded; this is not evidence of useful recovery.
    pub fn action_result(&mut self, workload: &Workload, outcome: Action) -> Result<(), RecoveryError> {
        let state = self
            .states
            .get(&workload.deployment)
            .ok_or_else(|| RecoveryError::Invalid("action target is not configured".into()))?;
        let Phase::Incident(index) = state.phase else {
            return Err(RecoveryError::Invalid("action has no incident".into()));
        };
        self.incidents
            .get_mut(index)
            .ok_or_else(|| RecoveryError::Invalid("incident state is absent".into()))?
            .actions
            .push(outcome);
        Ok(())
    }

    /// Serialize evidence with explicit unavailable measurements rather than zero-duration successful recoveries.
    #[must_use]
    pub fn report(&self) -> Report {
        let durations: Vec<_> = self
            .incidents
            .iter()
            .filter_map(|item| match item.recovered {
                ObservationTime::Recorded(time) => Some(time.saturating_sub(item.onset)),
                ObservationTime::Pending => None,
            })
            .collect();
        let detection: Vec<_> = self
            .incidents
            .iter()
            .filter_map(|item| match item.detected {
                ObservationTime::Recorded(time) => Some(time.saturating_sub(item.onset)),
                ObservationTime::Pending => None,
            })
            .collect();
        Report {
            schema_version: 1,
            job_id: self.config.job_id.clone(),
            mode: self.config.mode,
            clock_basis: "observer-monotonic-seconds",
            incidents: self.incidents.clone(),
            unknown_observations: self.unknown_observations,
            recovered_incidents: durations.len(),
            restart_budget: self.budget.clone(),
            mean_failure_detection_seconds: mean_seconds(&detection),
            mttr_seconds: mean_seconds(&durations),
            detected_incidents: detection.len(),
            failure_detection_seconds: detection,
            recovery_seconds: durations,
            checkpoint_measurements: self.checkpoints.clone(),
        }
    }
}

fn mean_seconds(values: &[u64]) -> MeanSeconds {
    let total = values.iter().fold(0_u64, |total, value| total.saturating_add(*value));
    // A process cannot practically retain more than u32::MAX incidents; checked_div preserves missing evidence as null.
    let mean = Duration::from_secs(total).checked_div(u32::try_from(values.len()).unwrap_or(u32::MAX));
    mean.map_or(MeanSeconds::Unavailable, |duration| {
        MeanSeconds::Measured(duration.as_secs_f64())
    })
}

#[expect(clippy::single_call_fn, reason = "separate wire validation from state transitions")]
fn event_evidence(event: &CloudEvent, job_id: &str) -> Result<(EventKind, Evidence), RecoveryError> {
    let evidence: Evidence = serde_json::from_value(event.data.clone())?;
    if evidence.schema_version != 1
        || evidence.job_id != job_id
        || evidence.instance.is_empty()
        || evidence.instance.len() > 128
        || evidence.sequence == 0
        || event.id.is_empty()
    {
        return Err(RecoveryError::Invalid("invalid lifecycle identity or job".into()));
    }
    let kind = event
        .event_type
        .strip_prefix("et.recovery.")
        .ok_or_else(|| RecoveryError::Invalid("not a recovery lifecycle event".into()))?;
    let kind = match kind {
        "started" => EventKind::Started,
        "progress" => EventKind::Progress,
        "failed" => EventKind::Failed,
        "completed" => EventKind::Completed,
        "checkpoint-saved" => EventKind::Checkpoint(CheckpointKind::Saved),
        "checkpoint-restored" => EventKind::Checkpoint(CheckpointKind::Restored),
        "resume-completed" => EventKind::Checkpoint(CheckpointKind::ResumeCompleted),
        _ => return Err(RecoveryError::Invalid("unsupported recovery lifecycle event".into())),
    };
    Ok((kind, evidence))
}

impl State {
    fn advance(
        &mut self,
        incident: &mut Incident,
        health: Health,
        unhealthy: bool,
        policy: &crate::config::Policy,
        now: u64,
    ) {
        if incident.status == Status::Suspected && !unhealthy {
            self.phase = Phase::Healthy;
            incident.status = Status::Transient;
            return;
        }
        if health == Health::Ready && !unhealthy && self.progress > incident.baseline {
            incident.recovered = ObservationTime::Recorded(now);
            incident.status = Status::Recovered;
            self.phase = Phase::Healthy;
            return;
        }
        if incident.status == Status::Suspected {
            if now.saturating_sub(incident.onset) < policy.detection_grace_seconds {
                return;
            }
            incident.status = Status::Detected;
            incident.detected = ObservationTime::Recorded(now);
            self.last_action = now;
        }
        if now.saturating_sub(incident.onset) >= policy.deadline_seconds {
            incident.status = Status::Unrecovered;
        }
    }
}
