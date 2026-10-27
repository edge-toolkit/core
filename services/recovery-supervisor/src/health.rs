//! Local, session-bound supervision-cycle evidence; no listener, credentials, or workload mutations in a probe.

// fs-err wraps filesystem operations but uses the standard library's lock error type.
use std::fs::TryLockError;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use fs_err as fs;
use serde::{Deserialize, Serialize};

use crate::RecoveryError;
use crate::config::Config;
use crate::persistence::StateStore;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Evidence {
    schema_version: u32,
    job_id: String,
    session: String,
    completed_at: SystemTime,
}

/// Own the budget lease and publish health only after a complete, durably recorded observation cycle.
#[derive(Debug)]
pub struct Session {
    // This guard holds the exclusive budget lock until the supervisor exits, including cancelled futures.
    lease: std::sync::Arc<fs::File>,
    store: StateStore,
    job_id: String,
    identity: String,
    observed: std::sync::atomic::AtomicBool,
}

impl Session {
    /// Acquire exclusive ownership before changing the session identity or publishing health evidence.
    pub fn acquire(config: &Config) -> Result<Self, RecoveryError> {
        config.validate()?;
        let lease = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(config.state_file.with_extension("lock"))?;
        lease.try_lock()?;
        let lease = std::sync::Arc::new(lease);
        let identity = uuid::Uuid::new_v4().to_string();
        // Windows mandatory locks reject reads through another handle with "os error 33". Keep the identity outside the
        // locked file, and replace it only after acquiring ownership.
        fs::write(identity_path(config), identity.as_bytes())?;
        Ok(Self {
            store: StateStore::owned(evidence_path(config), std::sync::Arc::clone(&lease)),
            lease,
            job_id: config.job_id.clone(),
            identity,
            observed: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Bind a context writer to this session so a replacement cannot overtake an unfinished committed save.
    #[must_use]
    pub fn state_store(&self, path: PathBuf) -> StateStore {
        StateStore::owned(path, std::sync::Arc::clone(&self.lease))
    }

    /// Refresh evidence only when the policy loop has finished observing, acting, and persisting its report.
    pub async fn advance(&self) -> Result<(), RecoveryError> {
        self.store
            .persist(Evidence {
                schema_version: 1,
                job_id: self.job_id.clone(),
                session: self.identity.clone(),
                completed_at: SystemTime::now(),
            })
            .await?;
        self.observed.store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }

    /// Keep an established observer alive during reconnection without making registration alone pass startup health.
    pub async fn maintain(&self) -> Result<(), RecoveryError> {
        if self.observed.load(std::sync::atomic::Ordering::Acquire) {
            self.advance().await?;
        }
        Ok(())
    }
}

/// Bound a full supervision cycle using configured command deadlines and I/O slack.
///
/// Includes polling, one observation and potential restart per workload, and ten seconds for persistence.
#[must_use]
pub fn silence_limit(config: &Config) -> Duration {
    let mut commands = config.workloads.iter().fold(0_u64, |budget, _workload| {
        budget.saturating_add(config.command_timeout_seconds.saturating_mul(2))
    });
    for name in config.resilience.dependency_names() {
        if !config.workloads.iter().any(|workload| workload.deployment == name) {
            commands = commands.saturating_add(config.command_timeout_seconds);
        }
    }
    Duration::from_secs(config.poll_seconds.saturating_add(commands).saturating_add(10))
}

/// Reject absent owners, old sessions, malformed evidence, stale cycles, and timestamps ahead of the local clock.
pub fn check(config: &Config) -> Result<(), RecoveryError> {
    config.validate()?;
    let session = current_session(config)?;
    let evidence: Evidence = serde_json::from_slice(&fs::read(evidence_path(config))?)?;
    evidence.validate(config, &session)
}

/// Require a live owner before reading the session identity; a stale lock file is not proof of liveness.
#[expect(
    clippy::single_call_fn,
    reason = "lease ownership is checked separately from completed-cycle evidence"
)]
fn current_session(config: &Config) -> Result<String, RecoveryError> {
    let lock_path = config.state_file.with_extension("lock");
    let lease = fs::OpenOptions::new().read(true).write(true).open(&lock_path)?;
    match lease.try_lock() {
        Ok(()) => return Err(RecoveryError::Invalid("recovery supervisor is not running".into())),
        Err(TryLockError::WouldBlock) => {}
        // Unsupported filesystems and other OS lock failures are errors, not evidence of a live owner.
        Err(error) => return Err(error.into()),
    }
    Ok(fs::read_to_string(identity_path(config))?)
}

impl Evidence {
    /// Match the live session and reject incomplete or expired supervision-cycle evidence.
    fn validate(&self, config: &Config, session: &str) -> Result<(), RecoveryError> {
        if self.schema_version != 1 || self.job_id != config.job_id || self.session != session {
            return Err(RecoveryError::Invalid(
                "no completed cycle for the current recovery session".into(),
            ));
        }
        let age = SystemTime::now().duration_since(self.completed_at)?;
        if age > silence_limit(config) {
            return Err(RecoveryError::Invalid("recovery supervision cycle is stale".into()));
        }
        Ok(())
    }
}

/// Keep the local heartbeat beside the durable budget without overwriting that budget.
fn evidence_path(config: &Config) -> PathBuf {
    config.state_file.with_extension("health.json")
}

/// Keep readable session metadata separate from the exclusively locked ownership file.
fn identity_path(config: &Config) -> PathBuf {
    config.state_file.with_extension("session")
}
