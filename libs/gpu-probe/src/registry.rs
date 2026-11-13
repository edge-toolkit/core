//! The registry of every signature the probes have been seen to give, and what it says a signature was seen on.

use std::sync::LazyLock;

use serde_json::Value;

use crate::PROBES;
use crate::sighting::{RowError, Sighting};

/// The registry: the JSON written from `signatures.yaml`, embedded as text when the crate is compiled.
///
/// The text is read into a value the first time anything is asked of the registry, rather than written out as code
/// for the compiler to build, because building a value this size in one function takes more stack than a thread can
/// be counted on to have. The file is in this crate's own directory, which is what the variable names, and a published
/// copy of the crate has no repository root to find it from instead.
#[expect(
    clippy::unwrap_used,
    reason = "the text is this crate's own file, which the task that writes it only ever writes as JSON"
)]
static REGISTRY: LazyLock<Value> = LazyLock::new(|| {
    // nosemgrep: no-semgrep-mentions, no-ast-grep-mentions -- Required compile-time path annotation.
    // ast-grep-ignore: no-cargo-manifest-dir
    let text = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/signatures.json"));
    serde_json::from_str(text).unwrap()
});

/// Every entry of the registry.
fn entries() -> impl Iterator<Item = &'static Value> {
    REGISTRY
        .get("signatures")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

/// Every registry entry holding the answers `words`, measured through `interface`.
///
/// An entry was measured through `interface` when it is filed under that name, or under it with `-split` added, which
/// is how a browser's signature is filed when each probe ran in a shader of its own, as this crate's do. An entry holds
/// the answers when it has a result under each probe's name equal to the word given for that probe, so an entry with no
/// results, kept for an interface a browser withheld, holds none.
fn holding<'words>(interface: &'words str, words: &'words [String]) -> impl Iterator<Item = &'static Value> + 'words {
    let split = format!("{interface}-split");
    entries()
        .filter(move |entry| {
            let filed = entry.get("interface").and_then(Value::as_str);
            filed == Some(interface) || filed == Some(split.as_str())
        })
        .filter(move |entry| {
            let results = entry.get("results");
            words.len() == PROBES.len()
                && PROBES.iter().zip(words).all(|(probe, word)| {
                    results.and_then(|held| held.get(probe.0)).and_then(Value::as_str) == Some(word.as_str())
                })
        })
}

/// Every row the entries `found` list as places they have been seen, in the order the entries come.
///
/// The first entry [`Sighting::read_entry`] refuses is what this fails with. Every entry handed here comes from the
/// registry compiled into this crate, which `every_row_the_registry_holds_can_be_read` proves readable, so the refusal
/// is passed through by `collect` rather than by a branch of this function's own that nothing could take.
fn rows(found: impl Iterator<Item = &'static Value>) -> Result<Vec<Sighting>, RowError> {
    found
        .map(Sighting::read_entry)
        .collect::<Result<Vec<_>, _>>()
        .map(|entries| entries.into_iter().flatten().collect())
}

/// The label of every registry entry holding the answers `words`, measured through `interface`.
#[expect(
    clippy::single_call_fn,
    reason = "one of the registry's questions, kept beside the data it is asked of"
)]
pub fn labels(interface: &str, words: &[String]) -> Vec<&'static str> {
    holding(interface, words)
        .filter_map(|entry| entry.get("label").and_then(Value::as_str))
        .collect()
}

/// Every place the answers `words`, measured through `interface`, have been seen.
#[expect(
    clippy::single_call_fn,
    reason = "one of the registry's questions, kept beside the data it is asked of"
)]
pub fn seen(interface: &str, words: &[String]) -> Result<Vec<Sighting>, RowError> {
    rows(holding(interface, words))
}

/// Every place the registry has seen any signature.
pub fn every_sighting() -> Result<Vec<Sighting>, RowError> {
    rows(entries())
}
