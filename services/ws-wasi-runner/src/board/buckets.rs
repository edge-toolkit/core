//! Agent storage kept in a board's memory, for a transport whose hub runs on the same board.

use alloc::collections::BTreeMap;
#[cfg(not(feature = "std"))]
use alloc::{string::String, vec::Vec};

/// Every storage bucket on one board, by agent id, each a map from key to value.
///
/// A handle: its clones are the same buckets, so every guest a board runs, and the hub beside them, can read what
/// another wrote, as agents share the hub's storage elsewhere.
#[derive(Clone, Default)]
pub struct Buckets(alloc::sync::Arc<spin::Mutex<BTreeMap<String, Stored>>>);

/// What one storage bucket holds, a map from key to value.
type Stored = BTreeMap<String, Vec<u8>>;

/// That a bucket holds no value under the key asked for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Absent;

impl Buckets {
    /// What `bucket` holds under `key`.
    ///
    /// # Errors
    ///
    /// [`Absent`] when the bucket holds nothing under `key`.
    pub fn get(&self, bucket: &str, key: &str) -> Result<Vec<u8>, Absent> {
        self.0
            .lock()
            .get(bucket)
            .and_then(|stored| stored.get(key))
            .cloned()
            .ok_or(Absent)
    }

    /// Hold `value` under `key` in `bucket`, replacing what was there.
    pub fn set(&self, bucket: &str, key: &str, value: Vec<u8>) {
        let _previous = self
            .0
            .lock()
            .entry(String::from(bucket))
            .or_default()
            .insert(String::from(key), value);
    }
}
