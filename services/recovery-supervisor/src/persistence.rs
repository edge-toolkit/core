//! Ordered state replacement without blocking the async transport or racing a cancelled save.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fs_err as fs;
use serde::Serialize;
use tokio::sync::Mutex;

use crate::RecoveryError;

/// One state file's ordered writer. Clones share the same write ordering and must be used for that file's saves.
#[derive(Clone, Debug)]
pub struct StateStore {
    path: PathBuf,
    serial: Arc<Mutex<()>>,
    ownership: Ownership,
}

#[derive(Clone, Debug)]
enum Ownership {
    Unbound,
    Guard { _lease: Arc<fs::File> },
}

impl StateStore {
    /// Bind a writer to its operator-configured state file without creating or modifying it.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            serial: Arc::new(Mutex::new(())),
            ownership: Ownership::Unbound,
        }
    }

    /// Keep the supervisor's lease until blocking persistence completes, even when its async caller is cancelled.
    #[must_use]
    pub fn owned(path: PathBuf, lease: Arc<fs::File>) -> Self {
        Self {
            ownership: Ownership::Guard { _lease: lease },
            ..Self::new(path)
        }
    }

    /// Serialize and atomically replace state on a blocking thread, completing before recovery may be requested.
    pub async fn persist<T>(&self, value: T) -> Result<(), RecoveryError>
    where
        T: Serialize + Send + 'static,
    {
        // The blocking task owns the permit: cancelling its waiter cannot let a later save overtake this write.
        let permit = Arc::clone(&self.serial).lock_owned().await;
        let path = self.path.clone();
        let ownership = self.ownership.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let _ownership = ownership;
            persist(&path, &value)
        })
        .await?
    }
}

/// Stage a flushed document and atomically commit it at the configured state path.
#[expect(
    clippy::single_call_fn,
    reason = "atomic filesystem operation is isolated from async scheduling"
)]
fn persist<T>(path: &Path, value: &T) -> Result<(), RecoveryError>
where
    T: Serialize,
{
    let absolute = std::path::absolute(path)?;
    let parent = absolute
        .parent()
        .ok_or_else(|| RecoveryError::Invalid("state path must name a file within a directory".into()))?;
    let bytes = serde_json::to_vec(value)?;
    let temporary = stage(parent, &bytes)?;
    let _file = temporary.persist(&absolute)?;
    // Persist the directory entry too: syncing only the payload does not make a rename crash-durable on Unix.
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

/// Flush a complete replacement before making it visible at the durable state's path.
#[expect(
    clippy::single_call_fn,
    reason = "staging a flushed file is distinct from committing its directory entry"
)]
fn stage(parent: &Path, bytes: &[u8]) -> Result<tempfile::NamedTempFile, RecoveryError> {
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    Ok(temporary)
}
