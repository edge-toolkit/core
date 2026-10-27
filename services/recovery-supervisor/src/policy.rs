//! Deterministic recovery decisions driven by observer time and application evidence, not by benchmark actions.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;
use std::time::SystemTime;

use edge_toolkit::ws::CloudEvent;
use serde::{Deserialize, Serialize};

#[cfg(test)]
// Compile-time inclusion keeps private guard tests under tests/ without exposing policy internals.
// nosemgrep: no-semgrep-mentions, no-ast-grep-mentions -- Required compile-time path annotation.
// ast-grep-ignore: no-relative-path-literal
#[path = "../tests/unit/policy_guards.rs"]
mod guard_tests;

use crate::RecoveryError;
use crate::config::{Config, Mode, Workload};
use crate::resilience::{Assessment, Classification, ObservationGap, Reason, Stability};

/// Infrastructure observations distinguish API failure from an unhealthy deployment.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[non_exhaustive]
#[serde(untagged)]
pub enum MeanSeconds {
    /// No eligible observations have been recorded; serialized as null.
    Unavailable,
    /// Arithmetic mean of eligible durations, in seconds.
    Measured(f64),
}

/// Owned report snapshot, safe to serialize off the async control task.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct Report {
    /// Persisted report format version.
    pub schema_version: u32,
    /// Job supervised by this observer.
    pub job_id: String,
    /// Whether external recovery actions are permitted.
    pub mode: Mode,
    /// Time coordinate used for incident observations.
    pub clock_basis: String,
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
    /// Latest condition and policy disposition per configured workload.
    pub assessments: BTreeMap<String, Assessment>,
    /// Periods without supervision, excluded from claims of continuous healthy observation.
    pub observation_gaps: Vec<ObservationGap>,
    /// Versioned restoration context owned by this supervisor, not an application checkpoint.
    pub context: Snapshot,
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
    /// Sustained health after first useful recovery; does not change the recovery timestamp.
    #[serde(default)]
    pub stability: Stability,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
enum Phase {
    #[default]
    Healthy,
    Incident(usize),
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
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
    fresh: bool,
    in_flight: bool,
}

/// Durable context is validated against the exact operator configuration before it can authorize actions.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Snapshot {
    schema_version: u32,
    configuration: Config,
    states: BTreeMap<String, State>,
    elapsed_seconds: u64,
    saved_at: SystemTime,
    last_reservation: ObservationTime,
    jitter_seed: u64,
    restored_sessions: u64,
    archived_reports: Vec<serde_json::Value>,
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
    assessments: BTreeMap<String, Assessment>,
    dependency_health: BTreeMap<String, Health>,
    gaps: Vec<ObservationGap>,
    observed_at: u64,
    time_offset: u64,
    last_reservation: ObservationTime,
    jitter_seed: u64,
    restored_sessions: u64,
    archived_reports: Vec<serde_json::Value>,
    connected: bool,
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
            assessments: BTreeMap::new(),
            dependency_health: BTreeMap::new(),
            gaps: Vec::new(),
            observed_at: 0,
            time_offset: 0,
            last_reservation: ObservationTime::Pending,
            jitter_seed: uuid::Uuid::now_v7().as_u64_pair().1,
            restored_sessions: 0,
            archived_reports: Vec::new(),
            connected: true,
        })
    }

    /// Restore committed policy state; legacy reports retain their budget and are archived without mixing clocks.
    pub fn restore(config: Config, value: serde_json::Value) -> Result<Self, RecoveryError> {
        let budget = serde_json::from_value(
            value
                .get("restart_budget")
                .cloned()
                .ok_or_else(|| RecoveryError::Invalid("state document has no restart budget".into()))?,
        )?;
        let mut supervisor = Self::new(config, budget)?;
        if value.get("context").is_none() {
            supervisor.archived_reports.push(value);
            return Ok(supervisor);
        }
        let report: Report = serde_json::from_value(value)?;
        supervisor.validate_context(&report)?;
        let snapshot = report.context;
        let Ok(downtime) = SystemTime::now().duration_since(snapshot.saved_at) else {
            return Err(RecoveryError::Invalid(format!(
                "future recovery saved_at {:?}; correct the system clock before retrying",
                snapshot.saved_at
            )));
        };
        let downtime = downtime.as_secs();
        supervisor.states = snapshot.states;
        supervisor.incidents = report.incidents;
        supervisor.checkpoints = report.checkpoint_measurements;
        supervisor.unknown_observations = report.unknown_observations;
        supervisor.assessments = report.assessments;
        supervisor.gaps = report.observation_gaps;
        supervisor.time_offset = snapshot.elapsed_seconds.saturating_add(downtime);
        supervisor.observed_at = supervisor.time_offset;
        supervisor.last_reservation = snapshot.last_reservation;
        supervisor.jitter_seed = snapshot.jitter_seed;
        supervisor.restored_sessions = snapshot.restored_sessions.saturating_add(1);
        supervisor.archived_reports = snapshot.archived_reports;
        supervisor.transport_lost(snapshot.elapsed_seconds, "supervisor restart");
        supervisor.transport_connected(supervisor.time_offset);
        Ok(supervisor)
    }

    /// Reject saved state from a different configuration before restoring any live policy state.
    fn validate_context(&self, report: &Report) -> Result<(), RecoveryError> {
        let snapshot = &report.context;
        if report.schema_version != 1
            || snapshot.schema_version != 1
            || report.job_id != self.config.job_id
            || report.mode != self.config.mode
            || serde_json::to_value(&snapshot.configuration)? != serde_json::to_value(&self.config)?
            || snapshot.states.keys().ne(self.states.keys())
        {
            return Err(RecoveryError::Invalid(
                "saved recovery context does not match configuration".into(),
            ));
        }
        Self::validate_saved_incidents(report)
    }

    /// Verify that each restored workload phase refers to its own recorded incident.
    #[expect(
        clippy::single_call_fn,
        reason = "incident references are a distinct persisted-state invariant"
    )]
    fn validate_saved_incidents(report: &Report) -> Result<(), RecoveryError> {
        for (name, state) in &report.context.states {
            if let Phase::Incident(index) = state.phase {
                let incident = report
                    .incidents
                    .get(index)
                    .ok_or_else(|| RecoveryError::Invalid("saved incident index is invalid".into()))?;
                if &incident.workload != name {
                    return Err(RecoveryError::Invalid("saved incident workload is inconsistent".into()));
                }
            }
        }
        Ok(())
    }

    /// Advance the job clock monotonically within a process, retaining measured downtime across replacement.
    #[must_use]
    pub const fn elapsed(&self, session_seconds: u64) -> u64 {
        self.time_offset.saturating_add(session_seconds)
    }

    /// Suspend actions while retaining budgets, incident baselines and accepted sequence numbers.
    pub fn transport_lost(&mut self, now: u64, reason: &str) {
        self.observed_at = self.observed_at.max(now);
        self.connected = false;
        if self
            .gaps
            .last()
            .is_none_or(|gap| gap.ended_at != ObservationTime::Pending)
        {
            self.gaps.push(ObservationGap {
                started_at: now,
                ended_at: ObservationTime::Pending,
                reason: reason.into(),
            });
        }
        for state in self.states.values_mut() {
            state.fresh = false;
        }
        for incident in &mut self.incidents {
            if matches!(incident.stability, Stability::Pending { .. }) {
                incident.stability = Stability::Incomplete;
            }
        }
        self.dependency_health.clear();
    }

    /// Resume observation after registration; active workloads must supply renewed lifecycle evidence before action.
    pub fn transport_connected(&mut self, now: u64) {
        self.observed_at = self.observed_at.max(now);
        self.connected = true;
        if let Some(gap) = self.gaps.last_mut() {
            gap.ended_at = ObservationTime::Recorded(now);
        }
    }

    /// Refresh the read-only dependency snapshot before evaluating any workload in an observation cycle.
    pub fn dependency_observation(&mut self, name: &str, health: Health) -> Result<(), RecoveryError> {
        if !self.config.resilience.dependency_names().contains(name) {
            return Err(RecoveryError::Invalid(
                "dependency observation is not configured".into(),
            ));
        }
        let _previous = self.dependency_health.insert(name.into(), health);
        Ok(())
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
        state.accept_evidence(kind, &evidence, &mut self.incidents, now)?;
        state.fresh = true;
        self.observed_at = self.observed_at.max(now);
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
        self.observed_at = self.observed_at.max(now);
        self.update_stability(workload, health, now)?;
        let state = self
            .states
            .get_mut(&workload.deployment)
            .ok_or_else(|| RecoveryError::Invalid("workload is not configured".into()))?;
        if health == Health::Unknown {
            self.unknown_observations = self.unknown_observations.saturating_add(1);
            self.assess(
                workload,
                Classification::ObservationUnknown,
                Reason::ObservationUnknown,
                now,
            );
            return Ok(Decision::Observe);
        }
        // An idle module is not stalled, and completing a job does not create another work obligation.
        let unhealthy = state.unhealthy(health, &self.config.policy, now);
        let classification = state.classify(health, unhealthy);
        if matches!(state.phase, Phase::Healthy) && unhealthy {
            let index = self.incidents.len();
            self.incidents
                .push(Incident::new(workload.deployment.clone(), now, state.progress));
            state.phase = Phase::Incident(index);
        }
        let Phase::Incident(index) = state.phase else {
            self.assess(workload, classification, Reason::Healthy, now);
            return Ok(Decision::Observe);
        };
        let incident = self
            .incidents
            .get_mut(index)
            .ok_or_else(|| RecoveryError::Invalid("incident state is absent".into()))?;
        state.advance(incident, health, unhealthy, &self.config.policy, now);
        if matches!(
            incident.status,
            Status::Recovered | Status::Transient | Status::Unrecovered
        ) {
            state.in_flight = false;
        }
        if incident.status == Status::Recovered && self.config.resilience.stability_seconds > 0 {
            incident.stability = Stability::Pending {
                since: now,
                progress: state.progress,
                instance: state.instance.clone(),
            };
        }
        let reason = self.eligibility(workload, now)?;
        self.assess(workload, classification, reason, now);
        Ok(if reason == Reason::RestartEligible {
            Decision::Restart
        } else {
            Decision::Observe
        })
    }

    /// Update sustained recovery independently of incident detection and intervention eligibility.
    fn update_stability(&mut self, workload: &Workload, health: Health, now: u64) -> Result<(), RecoveryError> {
        let state = self
            .states
            .get(&workload.deployment)
            .ok_or_else(|| RecoveryError::Invalid("workload is not configured".into()))?;
        let incident = self
            .incidents
            .iter_mut()
            .rev()
            .find(|item| item.workload == workload.deployment && item.status == Status::Recovered);
        if let Some(incident) = incident {
            if health == Health::Unknown && matches!(incident.stability, Stability::Pending { .. }) {
                incident.stability = Stability::Incomplete;
            } else {
                state.verify_stability(
                    incident,
                    state.unhealthy(health, &self.config.policy, now),
                    now,
                    self.config.resilience.stability_seconds,
                );
            }
        }
        Ok(())
    }

    /// Reserve an action before persisting the budget and executing the backend operation.
    pub fn reserve_restart(&mut self, workload: &Workload, now: u64) -> Result<(), RecoveryError> {
        if self.eligibility(workload, now)? != Reason::RestartEligible {
            return Err(RecoveryError::Invalid(
                "restart is not eligible under the configured policy".into(),
            ));
        }
        self.reserve_eligible_restart(workload, now)
    }

    /// Commit an admitted action while defending against missing or inconsistent reservation state.
    fn reserve_eligible_restart(&mut self, workload: &Workload, now: u64) -> Result<(), RecoveryError> {
        // Eligibility validated the target, phase and limits under this exclusive borrow. The following checks
        // remain fail-closed invariant guards if that validation or the reservation sequence changes later.
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
        Self::validate_reservation(&self.config, state, incident, *count, now)?;
        *count = count.saturating_add(1);
        state.last_action = now;
        self.observed_at = self.observed_at.max(now);
        state.in_flight = true;
        self.last_reservation = ObservationTime::Recorded(now);
        incident.actions.push(Action::RestartReserved);
        Ok(())
    }

    /// Recheck reservation invariants before consuming budget or changing the incident's action history.
    #[cfg_attr(
        not(test),
        expect(
            clippy::single_call_fn,
            reason = "independently tested fail-closed boundary before a restart reservation mutates state"
        )
    )]
    fn validate_reservation(
        config: &Config,
        state: &State,
        incident: &Incident,
        attempts: u32,
        now: u64,
    ) -> Result<(), RecoveryError> {
        if config.mode != Mode::ControllerAssisted
            || attempts >= config.policy.max_restarts
            || now.saturating_sub(state.last_action) < config.policy.restart_after_seconds
            || incident.status != Status::Detected
        {
            return Err(RecoveryError::Invalid(
                "restart is not eligible under the configured policy".into(),
            ));
        }
        Ok(())
    }

    /// Record whether a reserved backend request succeeded; this is not evidence of useful recovery.
    pub fn action_result(&mut self, workload: &Workload, outcome: Action) -> Result<(), RecoveryError> {
        let state = self
            .states
            .get_mut(&workload.deployment)
            .ok_or_else(|| RecoveryError::Invalid("action target is not configured".into()))?;
        let Phase::Incident(index) = state.phase else {
            return Err(RecoveryError::Invalid("action has no incident".into()));
        };
        let incident = self
            .incidents
            .get_mut(index)
            .ok_or_else(|| RecoveryError::Invalid("incident state is absent".into()))?;
        if matches!(outcome, Action::RestartFailed { .. }) {
            state.in_flight = false;
        }
        incident.actions.push(outcome);
        Ok(())
    }

    /// Evaluate safeguards again at reservation so a decision cannot bypass a newly consumed fleet limit.
    fn eligibility(&self, workload: &Workload, now: u64) -> Result<Reason, RecoveryError> {
        let state = self
            .states
            .get(&workload.deployment)
            .ok_or_else(|| RecoveryError::Invalid("restart target is not configured".into()))?;
        let Phase::Incident(index) = state.phase else {
            return Ok(Reason::Healthy);
        };
        let incident = self
            .incidents
            .get(index)
            .ok_or_else(|| RecoveryError::Invalid("incident state is absent".into()))?;
        let reason = self.incident_eligibility(state, incident, now);
        if reason != Reason::Healthy {
            return Ok(reason);
        }
        let dependency = self.dependency_reason(workload);
        if dependency != Reason::Healthy {
            return Ok(dependency);
        }
        Ok(self.restart_eligibility(workload, state, incident, now))
    }

    /// Require a detected incident with current evidence and time left for recovery.
    fn incident_eligibility(&self, state: &State, incident: &Incident, now: u64) -> Reason {
        if incident.status == Status::Unrecovered
            || now.saturating_sub(incident.onset) >= self.config.policy.deadline_seconds
        {
            return Reason::DeadlineExceeded;
        }
        if incident.status != Status::Detected {
            return Reason::DetectionGrace;
        }
        if !self.connected || (state.active && !state.completed && !state.fresh) {
            return Reason::EvidenceRequired;
        }
        Reason::Healthy
    }

    /// Apply intervention mode, per-workload limits and fleet coordination after evidence checks pass.
    fn restart_eligibility(&self, workload: &Workload, state: &State, incident: &Incident, now: u64) -> Reason {
        if self.config.mode != Mode::ControllerAssisted {
            return Reason::ObserveOnly;
        }
        let attempts = self
            .budget
            .restarts
            .get(&workload.deployment)
            .copied()
            .unwrap_or_default();
        if attempts >= self.config.policy.max_restarts {
            return Reason::BudgetExhausted;
        }
        let cooldown = self.cooldown(incident);
        if now.saturating_sub(state.last_action) < cooldown {
            return if state.in_flight {
                Reason::RecoveryInProgress
            } else {
                Reason::Cooldown
            };
        }
        self.fleet_eligibility(state, now)
    }

    /// Count outstanding useful-recovery requests, not merely concurrent kubectl child processes.
    fn fleet_eligibility(&self, state: &State, now: u64) -> Reason {
        let active = self
            .states
            .values()
            .filter(|candidate| {
                let Phase::Incident(index) = candidate.phase else {
                    return false;
                };
                candidate.in_flight
                    && self.incidents.get(index).is_some_and(|incident| {
                        // A restored terminal incident can retain a stale in-flight marker.
                        incident.status == Status::Detected
                            && now.saturating_sub(incident.onset) < self.config.policy.deadline_seconds
                    })
            })
            .count();
        if !state.in_flight
            && self.config.resilience.max_concurrent_recoveries > 0
            && u32::try_from(active).unwrap_or(u32::MAX) >= self.config.resilience.max_concurrent_recoveries
        {
            return Reason::ConcurrencyLimit;
        }
        if let ObservationTime::Recorded(previous) = self.last_reservation
            && now.saturating_sub(previous) < self.config.resilience.action_interval_seconds
        {
            return Reason::ActionInterval;
        }
        Reason::RestartEligible
    }

    /// Unknown dependencies suppress interventions without being labelled as confirmed failures.
    fn dependency_reason(&self, workload: &Workload) -> Reason {
        self.dependency_condition(&workload.deployment)
    }

    /// Evaluate the validated dependency graph transitively, using only this cycle's read-only observations.
    fn dependency_condition(&self, name: &str) -> Reason {
        let Some(dependencies) = self.config.resilience.dependencies.get(name) else {
            return Reason::Healthy;
        };
        let mut reason = Reason::Healthy;
        for dependency in dependencies {
            match self.dependency_health.get(dependency) {
                Some(Health::Ready) => {
                    let nested = self.dependency_condition(dependency);
                    if nested == Reason::DependencyUnavailable {
                        return nested;
                    }
                    if nested == Reason::DependencyUnknown {
                        reason = nested;
                    }
                    if self
                        .states
                        .get(dependency)
                        .is_some_and(|state| state.unhealthy(Health::Ready, &self.config.policy, self.observed_at))
                    {
                        return Reason::DependencyUnavailable;
                    }
                }
                Some(Health::Unready) => return Reason::DependencyUnavailable,
                Some(Health::Unknown) | None => reason = Reason::DependencyUnknown,
            }
        }
        reason
    }

    /// Publish a bounded latest assessment without changing incident or first-recovery measurements.
    fn assess(&mut self, workload: &Workload, mut classification: Classification, reason: Reason, now: u64) {
        if reason == Reason::DependencyUnavailable {
            classification = Classification::DependencyUnavailable;
        }
        let next_action_at = self.next_action_time(workload, reason);
        let _previous = self.assessments.insert(
            workload.deployment.clone(),
            Assessment {
                classification,
                reason,
                observed_at: now,
                next_action_at,
                dependencies: self
                    .config
                    .resilience
                    .dependencies
                    .get(&workload.deployment)
                    .cloned()
                    .unwrap_or_default(),
            },
        );
    }

    /// Report a scheduled retry only when evidence permits it; dependency and capacity waits have no promised time.
    fn next_action_time(&self, workload: &Workload, reason: Reason) -> ObservationTime {
        if !matches!(
            reason,
            Reason::Cooldown | Reason::RecoveryInProgress | Reason::ActionInterval | Reason::RestartEligible
        ) {
            return ObservationTime::Pending;
        }
        // These reasons follow successful incident validation. Retain defensive fallbacks for future call-site
        // changes rather than promising a retry time for inconsistent state.
        let Some(state) = self.states.get(&workload.deployment) else {
            return ObservationTime::Pending;
        };
        let Phase::Incident(index) = state.phase else {
            return ObservationTime::Pending;
        };
        let Some(incident) = self.incidents.get(index) else {
            return ObservationTime::Pending;
        };
        let cooldown = self.cooldown(incident);
        let mut next = state.last_action.saturating_add(cooldown);
        if let ObservationTime::Recorded(previous) = self.last_reservation {
            next = next.max(previous.saturating_add(self.config.resilience.action_interval_seconds));
        }
        if next >= incident.onset.saturating_add(self.config.policy.deadline_seconds) {
            return ObservationTime::Pending;
        }
        ObservationTime::Recorded(next)
    }

    /// Use the same persisted jitter sample for eligibility and the reported retry schedule.
    fn cooldown(&self, incident: &Incident) -> u64 {
        self.config.resilience.cooldown(
            self.config.policy.restart_after_seconds,
            incident.attempts(),
            &incident.workload,
            self.jitter_seed ^ incident.onset,
        )
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
            clock_basis: if self.restored_sessions == 0 {
                "observer-monotonic-seconds".into()
            } else {
                "observer-elapsed-seconds-with-restart-gaps".into()
            },
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
            assessments: self.assessments.clone(),
            observation_gaps: self.gaps.clone(),
            context: Snapshot {
                schema_version: 1,
                configuration: self.config.clone(),
                states: self.states.clone(),
                elapsed_seconds: self.observed_at,
                saved_at: SystemTime::now(),
                last_reservation: self.last_reservation,
                jitter_seed: self.jitter_seed,
                restored_sessions: self.restored_sessions,
                archived_reports: self.archived_reports.clone(),
            },
        }
    }
}

/// Represent missing observations separately from the mean duration of recorded events.
fn mean_seconds(values: &[u64]) -> MeanSeconds {
    let total = values.iter().fold(0_u64, |total, value| total.saturating_add(*value));
    // A process cannot practically retain more than u32::MAX incidents; checked_div preserves missing evidence as null.
    let mean = Duration::from_secs(total).checked_div(u32::try_from(values.len()).unwrap_or(u32::MAX));
    mean.map_or(MeanSeconds::Unavailable, |duration| {
        MeanSeconds::Measured(duration.as_secs_f64())
    })
}

/// Decode and validate lifecycle evidence before permitting a policy-state transition.
#[expect(clippy::single_call_fn, reason = "separate wire validation from state transitions")]
fn event_evidence(event: &CloudEvent, job_id: &str) -> Result<(EventKind, Evidence), RecoveryError> {
    let evidence: Evidence = serde_json::from_value(event.data.clone())?;
    evidence.validate(job_id, &event.id)?;
    let kind = event
        .event_type
        .strip_prefix("et.recovery.")
        .ok_or_else(|| RecoveryError::Invalid("not a recovery lifecycle event".into()))?;
    Ok((EventKind::parse(kind)?, evidence))
}

impl Evidence {
    /// Validate message identity independently of lifecycle event-name decoding.
    fn validate(&self, job_id: &str, event_id: &str) -> Result<(), RecoveryError> {
        if self.schema_version != 1
            || self.job_id != job_id
            || self.instance.is_empty()
            || self.instance.len() > 128
            || self.sequence == 0
            || event_id.is_empty()
        {
            return Err(RecoveryError::Invalid("invalid lifecycle identity or job".into()));
        }
        Ok(())
    }
}

impl EventKind {
    /// Decode only supported recovery lifecycle action names.
    #[expect(
        clippy::single_call_fn,
        reason = "wire event names have a single explicit decoding table"
    )]
    fn parse(name: &str) -> Result<Self, RecoveryError> {
        match name {
            "started" => Ok(Self::Started),
            "progress" => Ok(Self::Progress),
            "failed" => Ok(Self::Failed),
            "completed" => Ok(Self::Completed),
            "checkpoint-saved" => Ok(Self::Checkpoint(CheckpointKind::Saved)),
            "checkpoint-restored" => Ok(Self::Checkpoint(CheckpointKind::Restored)),
            "resume-completed" => Ok(Self::Checkpoint(CheckpointKind::ResumeCompleted)),
            _ => Err(RecoveryError::Invalid("unsupported recovery lifecycle event".into())),
        }
    }
}

impl State {
    /// Describe only conditions supported by the current deployment and application observations.
    fn classify(&self, health: Health, unhealthy: bool) -> Classification {
        if self.failed {
            Classification::ApplicationFailure
        } else if health == Health::Unready {
            Classification::DeploymentUnavailable
        } else if unhealthy {
            Classification::ProgressStalled
        } else {
            Classification::Healthy
        }
    }

    /// A quiet active workload needs additional progress; completed work needs only continued healthy observations.
    fn verify_stability(&self, incident: &mut Incident, unhealthy: bool, now: u64, window: u64) {
        let Stability::Pending {
            since,
            progress,
            instance,
        } = &incident.stability
        else {
            return;
        };
        if unhealthy || instance != &self.instance {
            incident.stability = Stability::Unstable { observed_at: now };
            return;
        }
        if now.saturating_sub(*since) >= window && (self.completed || self.progress > *progress) {
            incident.stability = Stability::Stable { verified_at: now };
        }
    }
    /// Adopt a first instance's baseline only after validating its sequence and identity.
    fn accept_evidence(
        &mut self,
        kind: EventKind,
        evidence: &Evidence,
        incidents: &mut [Incident],
        now: u64,
    ) -> Result<(), RecoveryError> {
        self.validate_evidence(kind, evidence)?;
        if self.instance.is_empty()
            && let Phase::Incident(index) = self.phase
        {
            incidents
                .get_mut(index)
                .ok_or_else(|| RecoveryError::Invalid("incident state is absent".into()))?
                .baseline = evidence.progress;
        }
        self.apply_evidence(kind, evidence, now);
        Ok(())
    }

    /// Reject retired instances, replayed sequence numbers and unannounced replacement processes.
    fn validate_evidence(&self, kind: EventKind, evidence: &Evidence) -> Result<(), RecoveryError> {
        let changing_instance = !self.instance.is_empty() && evidence.instance != self.instance;
        if self.retired.contains(&evidence.instance)
            || (evidence.instance == self.instance && evidence.sequence <= self.sequence)
            || (changing_instance && kind != EventKind::Started)
        {
            return Err(RecoveryError::Invalid(
                "stale evidence or instance must announce started first".into(),
            ));
        }
        Ok(())
    }

    /// Update process identity and useful-progress counters after evidence validation succeeds.
    fn apply_evidence(&mut self, kind: EventKind, evidence: &Evidence, now: u64) {
        if evidence.instance != self.instance {
            if self.instance.is_empty() {
                self.progress = evidence.progress;
            } else {
                let _inserted = self.retired.insert(self.instance.clone());
            }
        }
        self.instance.clone_from(&evidence.instance);
        self.sequence = evidence.sequence;
        let advances_work = matches!(kind, EventKind::Progress | EventKind::Completed);
        if !self.active || (advances_work && evidence.progress > self.progress) {
            self.progress_at = now;
        }
        self.active = true;
        if !matches!(kind, EventKind::Checkpoint(_)) {
            self.failed = kind == EventKind::Failed;
            self.completed = kind == EventKind::Completed;
        }
        if advances_work {
            self.progress = self.progress.max(evidence.progress);
        }
    }

    /// Distinguish real deployment failures and active-work stalls from idle or completed workloads.
    fn unhealthy(&self, health: Health, policy: &crate::config::Policy, now: u64) -> bool {
        health == Health::Unready
            || self.failed
            || (self.active && !self.completed && now.saturating_sub(self.progress_at) >= policy.stall_seconds)
    }

    /// Advance an incident through detection, useful recovery or deadline expiry using observer time.
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
        if self.fresh && health == Health::Ready && !unhealthy && self.progress > incident.baseline {
            // Late progress closes the active phase without rewriting a missed recovery deadline as success.
            if incident.status != Status::Unrecovered {
                incident.recovered = ObservationTime::Recorded(now);
                incident.status = Status::Recovered;
            }
            self.phase = Phase::Healthy;
        }
    }
}

impl Incident {
    /// Capture an incident baseline once, before detection and interventions can advance.
    #[expect(
        clippy::single_call_fn,
        reason = "incident creation captures its immutable initial measurements"
    )]
    const fn new(workload: String, onset: u64, baseline: u64) -> Self {
        Self {
            workload,
            onset,
            baseline,
            detected: ObservationTime::Pending,
            recovered: ObservationTime::Pending,
            status: Status::Suspected,
            actions: Vec::new(),
            stability: Stability::Disabled,
        }
    }

    /// Backoff counts attempts for this incident while the durable budget remains job-wide.
    fn attempts(&self) -> u32 {
        u32::try_from(
            self.actions
                .iter()
                .filter(|action| matches!(action, Action::RestartReserved))
                .count(),
        )
        .unwrap_or(u32::MAX)
    }
}
