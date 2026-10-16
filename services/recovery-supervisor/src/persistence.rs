//! Ordered state replacement without blocking the async transport or racing a cancelled save.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use tokio::sync::Mutex;

use crate::RecoveryError;

/// One state file's ordered writer. Clones share the same write ordering and must be used for that file's saves.
#[derive(Clone, Debug)]
pub struct StateStore {
    path: PathBuf,
    serial: Arc<Mutex<()>>,
}

impl StateStore {
    /// Bind a writer to its operator-configured state file without creating or modifying it.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            serial: Arc::new(Mutex::new(())),
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
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            persist(&path, &value)
        })
        .await?
    }
}

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
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&serde_json::to_vec(value)?)?;
    temporary.as_file().sync_all()?;
    let _file = temporary.persist(&absolute)?;
    Ok(())
}
