//! Rejects an entry in `config/typos.toml` that no longer suppresses anything.
//!
//! typos records nothing about which of its ignore entries matched, so an entry outlives the text it was written for
//! without a word: the file it covered is deleted, the phrase is reworded, and the exemption stays behind, quietly
//! blinding the checker to that spelling everywhere else. An entry is needed exactly when leaving it out makes typos
//! report a finding, so each one is judged that way -- by running typos over the checkout with a copy of the config
//! that omits that single entry.
//!
//! The copy is written under `target/` rather than beside the original. typos resolves `files.extend-exclude` against
//! the directory it runs in, not the config's own, so the move does not change what the copy excludes. The unmodified
//! copy is run first to prove it: a copy that is not clean as written would make every verdict after it meaningless.

use std::process::{Command, Output};

use command_error::CommandExt as _;
use fs_err as fs;
use toml::{Table, Value};

use crate::error::Error;

/// The config under judgement.
const CONFIG: &str = "config/typos.toml";

/// Where each trimmed copy is written; `target/` is gitignored, so typos never reads the copy as a source file.
const SCRATCH_DIR: &str = "target/repo-check";

/// The trimmed copy itself.
const SCRATCH: &str = "target/repo-check/typos.toml";

/// typos' exit status when it found typos, as distinct from 0 for a clean run and anything else for an error.
const TYPOS_FOUND: i32 = 2;

/// The `[default]` and `[type.<name>]` keys whose entries each suppress findings.
const ENGINE_KEYS: [&str; 5] = [
    "extend-identifiers",
    "extend-ignore-identifiers-re",
    "extend-ignore-re",
    "extend-ignore-words-re",
    "extend-words",
];

/// The `[files]` keys whose entries each suppress findings.
const FILES_KEYS: [&str; 1] = ["extend-exclude"];

/// Which member of an array or table one entry is.
enum Member {
    Index(usize),
    Key(String),
}

/// One suppressing entry: where it sits, which member it is, and how to name it in a report.
struct Entry {
    path: Vec<String>,
    member: Member,
    label: String,
}

/// Every suppressing entry in `config`, in file order within each key.
#[expect(
    clippy::single_call_fn,
    reason = "distinct step of the check; kept separate for readability"
)]
fn entries(config: &Table) -> Vec<Entry> {
    let mut containers: Vec<(Vec<String>, &[&str])> = vec![
        (vec!["files".to_owned()], &FILES_KEYS),
        (vec!["default".to_owned()], &ENGINE_KEYS),
    ];
    if let Some(Value::Table(types)) = config.get("type") {
        for name in types.keys() {
            containers.push((vec!["type".to_owned(), name.clone()], &ENGINE_KEYS));
        }
    }
    let mut found = Vec::new();
    'containers: for (table_path, keys) in containers {
        let mut table = config;
        for segment in &table_path {
            let Some(Value::Table(inner)) = table.get(segment) else {
                continue 'containers;
            };
            table = inner;
        }
        for key in keys {
            let mut path = table_path.clone();
            path.push((*key).to_owned());
            let shown = path.iter().fold(String::default(), |dotted, segment| {
                if dotted.is_empty() {
                    segment.clone()
                } else {
                    format!("{dotted}.{segment}")
                }
            });
            match table.get(*key) {
                Some(Value::Array(items)) => {
                    for (index, item) in items.iter().enumerate() {
                        let label = format!("{shown} {item}");
                        found.push(Entry {
                            path: path.clone(),
                            member: Member::Index(index),
                            label,
                        });
                    }
                }
                Some(Value::Table(words)) => {
                    for name in words.keys() {
                        let label = format!("{shown}.{name}");
                        found.push(Entry {
                            path: path.clone(),
                            member: Member::Key(name.clone()),
                            label,
                        });
                    }
                }
                _ => {}
            }
        }
    }
    found
}

/// `config` with `entry` left out.
///
/// The entry always exists, because [`entries`] read it from this same table; a path that did not resolve would leave
/// the copy whole, and the baseline run has already shown the whole copy to be clean, so the entry would be reported as
/// unneeded rather than silently passed.
#[expect(
    clippy::single_call_fn,
    reason = "distinct step of the check; kept separate for readability"
)]
fn without(config: &Table, entry: &Entry) -> Table {
    let mut copy = config.clone();
    let mut table = &mut copy;
    let Some((key, parents)) = entry.path.split_last() else {
        return copy;
    };
    for segment in parents {
        let Some(Value::Table(inner)) = table.get_mut(segment) else {
            return copy;
        };
        table = inner;
    }
    match (table.get_mut(key), &entry.member) {
        (Some(Value::Array(items)), Member::Index(index)) if *index < items.len() => {
            let _removed: Value = items.remove(*index);
        }
        (Some(Value::Table(words)), Member::Key(name)) => {
            let _removed: Option<Value> = words.remove(name);
        }
        _ => {}
    }
    copy
}

/// Whether typos finds nothing over the checkout under the config at [`SCRATCH`].
fn typos_clean(config: &Table) -> Result<bool, Error> {
    fs::write(SCRATCH, toml::to_string(config)?)?;
    let output: Output = Command::new("typos")
        .args(["--config", SCRATCH, "--format", "silent"])
        .output_checked_with(|output: &Output| match output.status.code() {
            Some(0_i32 | TYPOS_FOUND) => Ok(()),
            _ => Err(None::<String>),
        })?;
    Ok(output.status.success())
}

/// Reports every entry that suppresses nothing and returns how many there were.
#[expect(
    clippy::print_stdout,
    clippy::single_call_fn,
    reason = "one check of the repo-check CLI, called once by main; its findings on stdout are the tool's output"
)]
pub(crate) fn run() -> Result<usize, Error> {
    let config: Table = toml::from_str(&fs::read_to_string(CONFIG)?)?;
    fs::create_dir_all(SCRATCH_DIR)?;
    if !typos_clean(&config)? {
        println!("repo-check typos-ignores: typos reports findings with {CONFIG} as written; fix those first");
        return Ok(1);
    }
    let all = entries(&config);
    let mut unneeded = 0_usize;
    for entry in &all {
        if typos_clean(&without(&config, entry))? {
            println!("{CONFIG}: {} suppresses nothing; remove it", entry.label);
            unneeded = unneeded.saturating_add(1);
        }
    }
    let _removed: Result<(), std::io::Error> = fs::remove_file(SCRATCH);
    println!(
        "repo-check typos-ignores: {unneeded} of {} entries suppress nothing",
        all.len()
    );
    Ok(unneeded)
}
