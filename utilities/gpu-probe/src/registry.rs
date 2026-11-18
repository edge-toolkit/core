//! The registry a submission is added to: `libs/gpu-probe/signatures`, or a directory of the same shape.
//!
//! The directory holds `header.yaml`, the header; `curated/`, one file of entries per interface, named for it; and
//! `submitted/`, files of submissions proposed from the hosted probe page, waiting to be filed under `curated/`.
//!
//! A submission is one signature, or one interface a browser withheld, and the one place it was seen. When the file
//! of its interface already holds that signature, the place is added to the entry's `seen` rows, unless an identical
//! row is there already; otherwise the submission becomes a new entry at the end of the file, which is made if the
//! interface has none yet. The file is edited as text, a row inserted or an entry appended in the layout the file
//! already has, so its comments and every other entry stay byte for byte as they were.
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

/// The line a file of entries starts with, before its first entry.
const ENTRIES_START: &str = "signatures:\n";

/// The registry's header, in its directory: what a row may say, and the probe set the signatures answer.
const HEADER: &str = "header.yaml";

/// The directory of the registry's entries, one file per interface.
const CURATED: &str = "curated";

/// The directory of submissions proposed from the hosted probe page, waiting to be filed under [`CURATED`].
const SUBMITTED: &str = "submitted";

/// The directory of the lock file and staged copy of each file of entries.
///
/// They are kept apart so that [`CURATED`] holds the entries alone.
const WRITING: &str = "writing";

/// The repository's own registry directory, from the repository's root.
const REPOSITORY_REGISTRY: &str = "libs/gpu-probe/signatures";

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
    /// A file of entries does not open with the `signatures:` line every one starts with.
    #[error("a file of entries must start with a `signatures:` line")]
    NotEntries,
    /// An entry is filed under another interface than its own, in the file of the one named.
    #[error("an entry measured through {0} is filed under another interface")]
    Misfiled(String),
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
    /// It became a new entry, and its interface's file now holds this many.
    Added(usize),
    /// Its row was added to the entry with this label.
    Sighted(String),
    /// The entry with this label already held the same row, so nothing changed.
    Present(String),
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Added(entries) => write!(f, "added as entry {entries} of its interface"),
            Self::Sighted(label) => write!(f, "added a sighting to: {label}"),
            Self::Present(label) => write!(f, "already present: {label}"),
        }
    }
}

/// A file of entries, as far as matching a submission needs to read it.
#[derive(Default, Deserialize)]
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

/// One file of a registry's entries, which one request at a time adds to.
#[derive(Debug)]
pub struct Registry {
    /// The file.
    path: PathBuf,
    /// The lock file every writer of the file locks, in or out of this process.
    lock_file: PathBuf,
    /// Where each update is written before it is renamed over the file.
    staged: PathBuf,
    /// Held for the whole of each read, change and write, so that two submissions never interleave.
    lock: Mutex<()>,
}

impl Registry {
    /// The entries held in the file at `path`, which need not exist yet.
    ///
    /// Its lock file and staged copy are written beside it.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        let working = path.clone();
        Self::working_at(path, &working)
    }

    /// The entries held in the file at `path`, which need not exist yet, with its lock file and staged copy elsewhere.
    ///
    /// They are written at `working` with the extensions `yaml.lock` and `yaml.partial`. The staged copy is renamed
    /// over the file, so `working` must be on the same file system.
    #[must_use]
    pub fn working_at(path: PathBuf, working: &Path) -> Self {
        Self {
            path,
            lock_file: working.with_extension("yaml.lock"),
            staged: working.with_extension("yaml.partial"),
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
    /// which is left in place for the next writer to lock. An update that would not read back as the registry with just
    /// this one sighting added is refused, and the registry left as it was.
    pub fn register(&self, submission: &Submission) -> Result<Outcome, RegisterError> {
        let others = validate(submission)?;
        let _held = self.lock.lock()?;
        let lock_file = fs_err::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&self.lock_file)?;
        lock_file.file().lock()?;
        let (text, file) = match fs_err::read_to_string(&self.path) {
            Ok(text) => {
                let file: File = serde_saphyr::from_str(&text)?;
                (text, file)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (ENTRIES_START.to_owned(), File::default()),
            Err(error) => return Err(error.into()),
        };
        let row = row_text(&submission.seen, &others);
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
        // The update is written beside the registry and renamed over it, so the registry is never seen half written. A
        // staged file the rename did not take is removed; that removal is best effort, as a write that failed may have
        // left nothing to remove.
        let replaced = fs_err::write(&self.staged, updated).and_then(|()| fs_err::rename(&self.staged, &self.path));
        if replaced.is_err() {
            let _removed: Result<(), std::io::Error> = fs_err::remove_file(&self.staged);
        }
        replaced?;
        Ok(outcome)
    }
}

/// A file of submissions waiting in a registry's `submitted/` directory.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Waiting {
    /// Each submission, in the order it is filed.
    submissions: Vec<Submission>,
}

/// What is wrong with one file of a registry directory, or what stopped it being read or filed.
#[derive(Debug, thiserror::Error)]
#[error("{}: {problem}", path.display())]
#[non_exhaustive]
pub struct FileError {
    /// The file.
    pub path: PathBuf,
    /// What is wrong with it, boxed so an error is as small as a path and a pointer.
    pub problem: Box<RegisterError>,
}

impl FileError {
    /// A `problem` with the file at `path`.
    fn new<Problem>(path: &Path, problem: Problem) -> Self
    where
        Problem: Into<RegisterError>,
    {
        Self {
            path: path.to_owned(),
            problem: Box::new(problem.into()),
        }
    }
}

/// `result`, any problem it holds named as one with the file at `path`.
fn in_file<Value, Problem>(path: &Path, result: Result<Value, Problem>) -> Result<Value, FileError>
where
    Problem: Into<RegisterError>,
{
    match result {
        Ok(value) => Ok(value),
        Err(problem) => Err(FileError::new(path, problem)),
    }
}

/// A registry directory: its header, a file of entries per interface, and the submissions waiting to be filed.
#[derive(Debug)]
pub struct RegistryTree {
    /// The directory.
    dir: PathBuf,
}

impl RegistryTree {
    /// The registry held in the directory `dir`.
    #[must_use]
    pub const fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The repository's own registry, found from the working directory.
    #[must_use]
    pub fn repository() -> Self {
        Self::new(edge_toolkit::config::get_project_root().join(REPOSITORY_REGISTRY))
    }

    /// Add `submission` to the file of its interface's entries, as [`Registry::register`] does.
    pub fn register(&self, submission: &Submission) -> Result<Outcome, RegisterError> {
        // The interface is checked to be a name before it is made into a path, so it cannot name a file elsewhere.
        let _others: Vec<&str> = validate(submission)?;
        let file = format!("{}.yaml", submission.interface);
        // Only the working directory is made, so a registry directory that does not exist is an error, not created.
        let writing = self.dir.join(WRITING);
        match fs_err::create_dir(&writing) {
            Err(error) if error.kind() != std::io::ErrorKind::AlreadyExists => return Err(error.into()),
            Ok(()) | Err(_) => {}
        }
        Registry::working_at(self.dir.join(CURATED).join(&file), &writing.join(file)).register(submission)
    }

    /// The registry as one YAML document: the header, then every entry, their files taken in name order.
    pub fn text(&self) -> Result<String, FileError> {
        let header = self.dir.join(HEADER);
        let mut text = read(&header)?;
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(ENTRIES_START);
        for path in self.files(CURATED)? {
            let entries = read(&path)?;
            let body = in_file(
                &path,
                entries.strip_prefix(ENTRIES_START).ok_or(RegisterError::NotEntries),
            )?;
            text.push_str(body);
        }
        Ok(text)
    }

    /// Check every file but the header, answering how many submissions are waiting to be filed.
    ///
    /// Each entry must be filed under its own interface, and each waiting submission must be one the registry would
    /// take. That is what keeps a submission proposed from the hosted page from being merged unless it can be filed.
    pub fn check(&self) -> Result<usize, FileError> {
        for path in self.files(CURATED)? {
            let file: File = parse(&path)?;
            let own = path.file_stem().and_then(std::ffi::OsStr::to_str).unwrap_or_default();
            if let Some(stray) = file.signatures.iter().find(|entry| entry.interface != own) {
                return Err(FileError::new(&path, RegisterError::Misfiled(stray.interface.clone())));
            }
        }
        let mut waiting = 0_usize;
        for path in self.files(SUBMITTED)? {
            let submitted: Waiting = parse(&path)?;
            for submission in &submitted.submissions {
                let _others: Vec<&str> = in_file(&path, validate(submission))?;
            }
            waiting = waiting.saturating_add(submitted.submissions.len());
        }
        Ok(waiting)
    }

    /// File every waiting submission with [`Self::register`], removing each submitted file once all of it is filed.
    ///
    /// What became of each submission is given beside the file it came from. A submission refused stops the filing,
    /// and its file stays where it was; those before it in the file are filed already, and are found present when it is
    /// filed again.
    pub fn fold(&self) -> Result<Vec<(PathBuf, Vec<Outcome>)>, FileError> {
        let mut folded = Vec::new();
        for path in self.files(SUBMITTED)? {
            let submitted: Waiting = parse(&path)?;
            let filed: Result<Vec<_>, _> = submitted
                .submissions
                .iter()
                .map(|submission| self.register(submission))
                .collect();
            let outcomes = in_file(&path, filed)?;
            in_file(&path, fs_err::remove_file(&path))?;
            folded.push((path, outcomes));
        }
        Ok(folded)
    }

    /// Every YAML file in the directory `part` of the registry, in name order; none if there is no such directory.
    fn files(&self, part: &str) -> Result<Vec<PathBuf>, FileError> {
        let dir = self.dir.join(part);
        let listed = match fs_err::read_dir(&dir) {
            Ok(listed) => listed,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(FileError::new(&dir, error)),
        };
        let mut files = Vec::new();
        for item in listed {
            let path = in_file(&dir, item)?.path();
            if path.extension().is_some_and(|extension| extension == "yaml") {
                files.push(path);
            }
        }
        files.sort();
        Ok(files)
    }
}

/// The text of the file at `path`.
fn read(path: &Path) -> Result<String, FileError> {
    in_file(path, fs_err::read_to_string(path))
}

/// The file at `path`, read as YAML.
fn parse<Read>(path: &Path) -> Result<Read, FileError>
where
    Read: serde::de::DeserializeOwned,
{
    in_file(path, serde_saphyr::from_str(&read(path)?))
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
///
/// What it gives back is each field of the row whose value is outside the set the registry names for it.
fn validate(submission: &Submission) -> Result<Vec<&'static str>, RegisterError> {
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
    let sighting = Sighting::read(&Value::Object(row))?;
    for (field, value) in &submission.seen {
        check_text(field, value)?;
    }
    Ok(sighting.others())
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
///
/// A value from a field's fixed set is written bare, and every other value quoted, `others` among them: a value the
/// registry has no name for is free text, which may not read back as itself bare -- `null`, say, or `a: b`.
fn row_text(row: &BTreeMap<String, String>, others: &[&str]) -> String {
    let mut lines = String::default();
    for field in ROW_ORDER {
        if let Some(value) = row.get(field) {
            let lead = if lines.is_empty() { "      - " } else { "        " };
            let value = if ENUMERATED.contains(&field) && !others.contains(&field) {
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
