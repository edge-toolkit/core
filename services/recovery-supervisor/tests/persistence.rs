//! Ordered blocking writes preserve state on errors and do not race cancelled async callers.
#![cfg(test)]

use std::sync::Arc;
use std::time::Duration;

use et_recovery_supervisor::persistence::StateStore;
use fs_err as fs;
use serde::{Serialize, Serializer};
use tokio::sync::Notify;

struct PausedDocument {
    started: Arc<Notify>,
    release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
}

impl Serialize for PausedDocument {
    /// Pause serialization until the test releases the blocking writer.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.started.notify_one();
        let release = match self.release.lock() {
            Ok(release) => release,
            Err(error) => return Err(serde::ser::Error::custom(error)),
        };
        match release.recv_timeout(Duration::from_secs(5)) {
            Ok(()) => serializer.serialize_u64(1),
            Err(error) => Err(serde::ser::Error::custom(error)),
        }
    }
}

/// Keep cancelled callers from allowing older blocking writes to overtake later state commits.
#[tokio::test]
async fn cancellation_cannot_let_an_older_save_overwrite_the_final_report() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.json");
    let store = StateStore::new(path.clone());
    let writer = store.clone();
    let started = Arc::new(Notify::new());
    let (release, receiver) = std::sync::mpsc::channel();
    let paused = PausedDocument {
        started: Arc::clone(&started),
        release: std::sync::Mutex::new(receiver),
    };
    let first = tokio::spawn(async move { writer.persist(paused).await });
    // This single-thread runtime can still process notifications while serialization is deliberately blocked.
    tokio::time::timeout(Duration::from_secs(3), started.notified())
        .await
        .unwrap();
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    let mut final_save = tokio::spawn(async move { store.persist(2_u64).await });
    let _blocked = tokio::time::timeout(Duration::from_millis(20), &mut final_save)
        .await
        .unwrap_err();
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), final_save)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"2");
}

struct InvalidDocument;

impl Serialize for InvalidDocument {
    /// Inject a serialization failure before a replacement document can be committed.
    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Err(serde::ser::Error::custom("intentional serialization failure"))
    }
}

/// Preserve committed state after serialization failure and permit a subsequent successful save.
#[tokio::test]
async fn failed_serialization_preserves_the_last_document_and_releases_the_writer() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.json");
    let store = StateStore::new(path.clone());
    store.persist(1_u64).await.unwrap();
    let error = store.persist(InvalidDocument).await.unwrap_err();
    assert!(error.to_string().contains("intentional serialization failure"));
    assert_eq!(fs::read(&path).unwrap(), b"1");
    store.persist(2_u64).await.unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"2");
}
