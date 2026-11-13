//! The registry a submission is added to: `libs/gpu-probe/signatures.yaml`, or a file of the same shape.
//!
//! A submission is one signature, or one interface a browser withheld, and the one place it was seen. When the registry
//! already holds that signature through that interface, the place is added to the entry's `seen` rows, unless an
//! identical row is there already; otherwise the submission becomes a new entry at the end of the file. The file is
//! edited as text, a row inserted or an entry appended in the layout the file already has, so its comments, its header
//! and every other entry stay byte for byte as they were.
#![expect(
    clippy::single_call_fn,
    reason = "each step of Registry::register is a named function for readability; most are called from one place"
)]

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use et_gpu_probe::{RowError, Sighting};
use serde::Deserialize;
use serde_json::Value;

/// The most results one signature may hold.
const MAX_RESULTS: usize = 256;

/// The longest a name may be: an interface's, or a result's.
const MAX_NAME: usize = 64;

/// The longest a result's value may be.
const MAX_VALUE: usize = 128;

/// The longest free text may be: a label, a reason, or a row's free-text field.
const MAX_TEXT: usize = 200;

/// The line each entry of the registry starts with, before the interface it was measured through.
const ENTRY_START: &str = "  - interface:";

/// Every field a row may carry, in the order the registry writes them.
const ROW_ORDER: [&str; 18] = [
    "browser",
    "browser_version",
    "os",
    "os_version",
    "cpu_arch",
    "cpu_version",
    "gpu",
    "gpu_version",
    "driver_version",
    "api_version",
    "backend",
    "device",
    "runtime",
    "defence",
    "reported",
    "window",
    "date",
    "source",
];

/// The fields of a row whose values come from fixed sets, which the registry writes unquoted.
const ENUMERATED: [&str; 6] = ["browser", "os", "cpu_arch", "gpu", "backend", "window"];

/// What is wrong with a submission, or what stopped it being added.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RegisterError {
    /// The interface is not a name.
    #[error("interface must be a name of letters, digits, '_', '.' and '-', starting with a letter")]
    Interface,
    /// A free-text field is empty, too long, or holds a control character.
    #[error("{0} must be 1 to {MAX_TEXT} characters with no control characters")]
    Text(String),
    /// A signature's results are missing, too many, or not names beside plain values.
    #[error("results must map 1 to {MAX_RESULTS} names to values of letters, digits and '?_.+-'")]
    Results,
    /// A withheld interface carries results, or a signature a reason.
    #[error("a blocked interface takes a reason and no results, and a signature results and no reason")]
    Blocked,
    /// The row of where the submission was seen is not one the registry can read.
    #[error("seen: {0}")]
    Row(#[from] RowError),
    /// The registry file could not be read or written.
    #[error("the registry could not be read or written: {0}")]
    Io(#[from] std::io::Error),
    /// The registry file is not the YAML it should be.
    #[error("the registry is not valid YAML: {0}")]
    Parse(#[from] serde_saphyr::Error),
    /// The registry's text is not laid out as the YAML says it is, so there is nowhere to insert a row.
    #[error("the registry's entry {0} has no `results:` or `blocked:` line to insert a row before")]
    Layout(usize),
    /// Adding to entry `0` (a new one, if past the last) in the registry's text would not read back as that addition.
    #[error("the registry's layout around entry {0} is not one a sighting can be added to; it was left as it was")]
    Unwritable(usize),
    /// Another request panicked while it held the registry.
    #[error("the registry lock was poisoned by a failed request")]
    Poisoned,
}

impl RegisterError {
    /// Whether the submission itself is at fault, as against the registry it was to be added to.
    #[must_use]
    pub const fn is_rejection(&self) -> bool {
        matches!(
            self,
            Self::Interface | Self::Text(_) | Self::Results | Self::Blocked | Self::Row(_)
        )
    }
}

impl<Guard> From<std::sync::PoisonError<Guard>> for RegisterError {
    fn from(_poisoned: std::sync::PoisonError<Guard>) -> Self {
        Self::Poisoned
    }
}

/// One signature, or one interface a browser withheld, beside the one place it was seen.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Submission {
    /// The interface the signature was measured through, as the registry files it.
    pub interface: String,
    /// What the entry is, in a line.
    pub label: String,
    /// Each result's name beside its value, in the order the mechanism gives them; empty for a withheld interface.
    #[serde(default)]
    pub results: Results,
    /// Whether the interface was withheld, so that there are no results.
    #[serde(default)]
    pub blocked: bool,
    /// Why the interface was withheld; empty for a signature.
    #[serde(default)]
    pub reason: String,
    /// Where it was seen, as a row of the registry.
    pub seen: BTreeMap<String, String>,
}

impl Submission {
    /// A signature measured through `interface`, filed as `label` if it is new, with `results`, seen at `seen`.
    #[must_use]
    pub fn signature(
        interface: &str,
        label: &str,
        results: Vec<(String, String)>,
        seen: BTreeMap<String, String>,
    ) -> Self {
        Self {
            interface: interface.to_owned(),
            label: label.to_owned(),
            results: Results(results),
            blocked: false,
            reason: String::default(),
            seen,
        }
    }
}

/// A signature's results, each name beside its value, kept in the order they were given.
///
/// The registry lists an entry's results in the order its mechanism gives them -- the probes' output order -- so a new
/// entry is written in the order the submission held them, where a sorted map would lose it.
#[derive(Debug, Default)]
pub struct Results(Vec<(String, String)>);

impl Results {
    /// The results keyed by name, which is how two signatures are compared: order does not make them different.
    fn by_name(&self) -> BTreeMap<String, String> {
        self.0.iter().cloned().collect()
    }
}

impl<'de> Deserialize<'de> for Results {
    fn deserialize<Input>(deserializer: Input) -> Result<Self, Input::Error>
    where
        Input: serde::Deserializer<'de>,
    {
        deserializer.deserialize_map(InOrder)
    }
}

/// Reads a map into [`Results`] one pair at a time, which keeps the pairs in order.
struct InOrder;

impl<'de> serde::de::Visitor<'de> for InOrder {
    type Value = Results;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a map of result names to values")
    }

    fn visit_map<Pairs>(self, mut map: Pairs) -> Result<Results, Pairs::Error>
    where
        Pairs: serde::de::MapAccess<'de>,
    {
        let mut results = Vec::new();
        while let Some(pair) = map.next_entry()? {
            results.push(pair);
        }
        Ok(Results(results))
    }
}

/// What became of a submission that was accepted.
#[derive(Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Outcome {
    /// It became a new entry, and the registry now holds this many.
    Added(usize),
    /// Its row was added to the entry with this label.
    Sighted(String),
    /// The entry with this label already held the same row, so nothing changed.
    Present(String),
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Added(entries) => write!(f, "added; the registry now holds {entries} entries"),
            Self::Sighted(label) => write!(f, "added a sighting to: {label}"),
            Self::Present(label) => write!(f, "already present: {label}"),
        }
    }
}

/// The registry, as far as matching a submission needs to read it.
#[derive(Deserialize)]
struct File {
    /// Every entry, in file order.
    #[serde(default)]
    signatures: Vec<Entry>,
}

/// One entry of the registry, as far as matching a submission needs to read it.
#[derive(Deserialize, PartialEq)]
struct Entry {
    /// The interface it was measured through.
    interface: String,
    /// What it is, in a line.
    label: String,
    /// Each result's name beside its value.
    #[serde(default)]
    results: BTreeMap<String, String>,
    /// Whether it records a withheld interface.
    #[serde(default)]
    blocked: bool,
    /// Every place it was seen.
    #[serde(default)]
    seen: Vec<BTreeMap<String, String>>,
}

impl Entry {
    /// Whether `submission` is a sighting of this entry: the same signature, or the same withheld interface.
    fn holds(&self, submission: &Submission) -> bool {
        self.interface == submission.interface
            && self.blocked == submission.blocked
            && if submission.blocked {
                self.label == submission.label
            } else {
                self.results == submission.results.by_name()
            }
    }
}

/// A registry file, which one request at a time adds to.
#[derive(Debug)]
pub struct Registry {
    /// The file.
    path: PathBuf,
    /// Held for the whole of each read, change and write, so that two submissions never interleave.
    lock: Mutex<()>,
}

impl Registry {
    /// The registry held in the file at `path`.
    #[must_use]
    pub const fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    /// The file the registry is held in.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Add `submission` to the registry, once it is valid.
    ///
    /// The whole read, change and write is held against every other writer: another request through this `Registry`,
    /// and another process -- a `gpu-probe` run beside a running server -- through a lock file beside the registry,
    /// which is left in place for the next writer to lock. An update that would not read back as the registry with
    /// just this one sighting added is refused, and the registry left as it was.
    pub fn register(&self, submission: &Submission) -> Result<Outcome, RegisterError> {
        validate(submission)?;
        let _held = self.lock.lock()?;
        let lock_file = fs_err::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(self.path.with_extension("yaml.lock"))?;
        lock_file.file().lock()?;
        let text = fs_err::read_to_string(&self.path)?;
        let file: File = serde_saphyr::from_str(&text)?;
        let row = row_text(&submission.seen);
        let held = file.signatures.iter().position(|entry| entry.holds(submission));
        let (updated, outcome, index) = if let Some(index) = held {
            let entry = file.signatures.get(index).ok_or(RegisterError::Layout(index))?;
            if entry.seen.iter().any(|row| covers(row, &submission.seen)) {
                return Ok(Outcome::Present(entry.label.clone()));
            }
            let sighted = Outcome::Sighted(entry.label.clone());
            (insert_row(&text, index, &row)?, sighted, index)
        } else {
            let added = Outcome::Added(file.signatures.len().saturating_add(1));
            (append_entry(&text, submission, &row), added, file.signatures.len())
        };
        reads_back(&updated, &file, index)?;
        // The update is written beside the registry and renamed over it, so the registry is never seen half written.
        // A staged file the rename did not take is removed; that removal is best effort, as a write that failed may
        // have left nothing to remove.
        let staged = self.path.with_extension("yaml.partial");
        let replaced = fs_err::write(&staged, updated).and_then(|()| fs_err::rename(&staged, &self.path));
        if replaced.is_err() {
            let _removed: Result<(), std::io::Error> = fs_err::remove_file(&staged);
        }
        replaced?;
        Ok(outcome)
    }
}

/// Check `updated` reads back as `before` with only the one row added.
///
/// That row is in entry `index`, a new last entry if past the end, and every other entry reads back as it was.
///
/// The update is made by editing text, which assumes the layout this server writes. A registry edited by hand into
/// another one -- an empty `signatures: []` or `seen: []` list, say -- would otherwise be written over with YAML that
/// no longer parses, or parses as something else.
fn reads_back(updated: &str, before: &File, index: usize) -> Result<(), RegisterError> {
    let rows = |file: &File| file.signatures.get(index).map_or(0, |entry| entry.seen.len());
    let entries = before
        .signatures
        .len()
        .saturating_add(usize::from(index == before.signatures.len()));
    let Ok(after) = serde_saphyr::from_str::<File>(updated) else {
        return Err(RegisterError::Unwritable(index));
    };
    // Every other entry must read back as it did: one whose `seen` list is a YAML alias of the one written to would
    // otherwise gain the row too, a sighting of a signature nobody submitted.
    let others_unchanged = after
        .signatures
        .iter()
        .zip(&before.signatures)
        .enumerate()
        .all(|(position, (now, was))| position == index || now == was);
    if after.signatures.len() == entries && rows(&after) == rows(before).saturating_add(1) && others_unchanged {
        return Ok(());
    }
    Err(RegisterError::Unwritable(index))
}

/// Whether `row`, already in the registry, records `seen`: it gives every field `seen` gives, with the same value.
///
/// A row written by hand often carries more than a submission can know -- the machine's name, the OS release -- and a
/// submission that tells it nothing new is a sighting already on record, not one to add beside it.
fn covers(row: &BTreeMap<String, String>, seen: &BTreeMap<String, String>) -> bool {
    seen.iter().all(|(field, value)| row.get(field) == Some(value))
}

/// Check every field of `submission` is one the registry can hold, as the layout the text is written in needs.
fn validate(submission: &Submission) -> Result<(), RegisterError> {
    if !is_name(&submission.interface) {
        return Err(RegisterError::Interface);
    }
    check_text("label", &submission.label)?;
    let results = &submission.results.0;
    if submission.blocked {
        if !results.is_empty() {
            return Err(RegisterError::Blocked);
        }
        check_text("reason", &submission.reason)?;
    } else {
        if !submission.reason.is_empty() {
            return Err(RegisterError::Blocked);
        }
        let plain = results.iter().all(|(name, value)| is_name(name) && is_value(value));
        let distinct = submission.results.by_name().len() == results.len();
        if results.is_empty() || results.len() > MAX_RESULTS || !plain || !distinct {
            return Err(RegisterError::Results);
        }
    }
    let row: serde_json::Map<String, Value> = submission
        .seen
        .iter()
        .map(|(field, value)| (field.clone(), Value::String(value.clone())))
        .collect();
    let _sighting: Sighting = Sighting::read(&Value::Object(row))?;
    for (field, value) in &submission.seen {
        check_text(field, value)?;
    }
    Ok(())
}

/// Whether `text` is a name: a letter, then up to [`MAX_NAME`] letters, digits, `_`, `.` and `-` in all.
fn is_name(text: &str) -> bool {
    text.len() <= MAX_NAME
        && text.chars().next().is_some_and(|first| first.is_ascii_alphabetic())
        && text
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_.-".contains(character))
}

/// Whether `text` is a plain value: 1 to [`MAX_VALUE`] letters, digits, `?`, `_`, `.`, `+` and `-`.
fn is_value(text: &str) -> bool {
    (1..=MAX_VALUE).contains(&text.len())
        && text
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "?_.+-".contains(character))
}

/// Check `text`, the value of `field`, is free text the registry can quote.
fn check_text(field: &str, text: &str) -> Result<(), RegisterError> {
    let length = text.chars().count();
    if length == 0 || length > MAX_TEXT || text.chars().any(char::is_control) {
        return Err(RegisterError::Text(field.to_owned()));
    }
    Ok(())
}

/// `text` as a YAML double-quoted scalar, in ASCII alone.
///
/// The registry is a tracked file, which holds ASCII only, so a character past ASCII -- an accent in a label, say -- is
/// written as YAML's `\u` or `\U` escape for it, which reads back as the same character.
fn quoted(text: &str) -> String {
    let body: String = text
        .chars()
        .map(|character| match character {
            '\\' => "\\\\".to_owned(),
            '"' => "\\\"".to_owned(),
            ascii if ascii.is_ascii() => ascii.to_string(),
            wide if u32::from(wide) > 0xffff => format!("\\U{:08x}", u32::from(wide)),
            other => format!("\\u{:04x}", u32::from(other)),
        })
        .collect();
    format!("\"{body}\"")
}

/// The lines of `row` as an item of an entry's `seen` list, its fields in the registry's order.
fn row_text(row: &BTreeMap<String, String>) -> String {
    let mut lines = String::default();
    for field in ROW_ORDER {
        if let Some(value) = row.get(field) {
            let lead = if lines.is_empty() { "      - " } else { "        " };
            let value = if ENUMERATED.contains(&field) {
                value.clone()
            } else {
                quoted(value)
            };
            lines.push_str(&[lead, field, ": ", &value, "\n"].concat());
        }
    }
    lines
}

/// `text` with `row` inserted at the end of the `seen` list of its entry numbered `index`.
///
/// An entry's `seen` list ends where its `results:` or, for a withheld interface, its `blocked:` line starts.
fn insert_row(text: &str, index: usize, row: &str) -> Result<String, RegisterError> {
    let mut offset = 0_usize;
    let mut entries = 0_usize;
    let mut inside = false;
    for line in text.split_inclusive('\n') {
        let starts_entry = line.starts_with(ENTRY_START);
        if starts_entry {
            inside = entries == index;
            entries = entries.saturating_add(1);
        }
        let ends_seen = line.starts_with("    results:") || line.starts_with("    blocked:");
        if inside && !starts_entry && ends_seen {
            let (before, after) = text.split_at(offset);
            return Ok([before, row, after].concat());
        }
        offset = offset.saturating_add(line.len());
    }
    Err(RegisterError::Layout(index))
}

/// `text` with `submission`, seen at `row`, appended as a new entry.
fn append_entry(text: &str, submission: &Submission, row: &str) -> String {
    let mut updated = text.to_owned();
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    let label = quoted(&submission.label);
    updated.push_str(
        &[
            ENTRY_START,
            " ",
            &submission.interface,
            "\n    label: ",
            &label,
            "\n    seen:\n",
            row,
        ]
        .concat(),
    );
    if submission.blocked {
        updated.push_str(&["    blocked: true\n    reason: ", &quoted(&submission.reason), "\n"].concat());
    } else {
        updated.push_str("    results:\n");
        for (name, value) in &submission.results.0 {
            updated.push_str(&["      ", name, ": ", &quoted(value), "\n"].concat());
        }
    }
    updated
}
