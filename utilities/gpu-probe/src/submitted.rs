//! A file of submissions in the registry's `submitted/` directory, as the hosted probe page proposes one on GitHub.
//!
//! The page has no server to send to, so it puts the file in the link to GitHub's new-file page, which opens only a
//! link short enough. The file is therefore plain text holding little but the results' values, laid out so that a visit
//! with two GPUs fits in one link.
//!
//! A file is one block per GPU, the blocks separated by a blank line. A block's lines are, in order: `label`, then the
//! label each new entry is given; each field of the row the GPU was seen at, its name then its value; then one line
//! per interface, its name then its results' values in the order [`result_names`] gives their names, or its name, then
//! `blocked:` and the reason the browser withheld it. A name is separated from what follows by one space. A block with
//! no interface line is refused, so that one can never be filed as nothing.
//!
//! A line longer than the 120 characters a tracked file allows is broken. A line that starts with three spaces
//! continues the one before it with nothing between them, which breaks a word too long to fit; failing that, a line
//! that starts with two spaces continues it with a space between them. A tracked file holds ASCII alone, so a backslash
//! is written `\\`, and a character past ASCII as `\u{...}`, its code point in hex.
//!
//! [`write()`] lays submissions out as such a file, as the page does, for a measurement that has no registry it may
//! write to either, such as one taken on a CI runner.
#![expect(
    clippy::single_call_fn,
    reason = "each step of parse and write is a named function for readability, called from one place"
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use et_gpu_probe::result_names;
use sha2::{Digest as _, Sha256};

use crate::registry::{ROW_ORDER, RegisterError, Submission};

/// The longest line a tracked file may hold.
const LONGEST_LINE: usize = 120;

/// How many hex digits of its SHA-256 a file is named with, as the page names one: typos passes over a run this long.
const NAME_DIGITS: usize = 32;

/// What opens a line that continues the one before it, after a space.
const CONTINUED: &str = "  ";

/// What opens a line that continues the one before it with nothing between them.
const JOINED: &str = "   ";

/// What follows an interface's name on the line of an interface the browser withheld, before the reason.
const BLOCKED: &str = "blocked: ";

/// The name of the line holding a block's label.
const LABEL: &str = "label";

/// A line of a file once its continuations are joined to it, numbered as the line it starts on.
struct Line {
    /// The number of the line it starts on, counting from 1.
    number: usize,
    /// Its text, continuations joined on.
    text: String,
}

/// Every submission the file `text` holds, in file order.
///
/// # Errors
///
/// [`RegisterError::Submitted`] naming the line at fault where the file is not laid out as the module says. Whether
/// each submission is one the registry would take is left to [`crate::registry::RegistryTree::check`].
pub fn parse(text: &str) -> Result<Vec<Submission>, RegisterError> {
    let mut submissions = Vec::new();
    for block in blocks(text)? {
        submissions.extend(parse_block(&block)?);
    }
    Ok(submissions)
}

/// The blocks of `text`, each its lines with their continuations joined on.
fn blocks(text: &str) -> Result<Vec<Vec<Line>>, RegisterError> {
    let mut blocks = Vec::new();
    let mut block: Vec<Line> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let number = index.saturating_add(1);
        if line.is_empty() {
            if !block.is_empty() {
                blocks.push(std::mem::take(&mut block));
            }
        } else if let Some(rest) = line.strip_prefix(JOINED) {
            continue_last(&mut block, number, "", rest)?;
        } else if let Some(rest) = line.strip_prefix(CONTINUED) {
            continue_last(&mut block, number, " ", rest)?;
        } else {
            block.push(Line {
                number,
                text: line.to_owned(),
            });
        }
    }
    if !block.is_empty() {
        blocks.push(block);
    }
    Ok(blocks)
}

/// Add `rest`, from the line numbered `number`, to the last line of `block`, with `between` between them.
fn continue_last(block: &mut [Line], number: usize, between: &str, rest: &str) -> Result<(), RegisterError> {
    let continued = block
        .last_mut()
        .ok_or_else(|| fault(number, "a continuation with no line to continue"))?;
    continued.text.push_str(between);
    continued.text.push_str(rest);
    Ok(())
}

/// The submissions of one block: one for each of its interface lines, each with the block's label and row.
fn parse_block(block: &[Line]) -> Result<Vec<Submission>, RegisterError> {
    let mut label = String::default();
    let mut seen = BTreeMap::new();
    let mut submissions = Vec::new();
    for line in block {
        let (name, rest) = line.text.split_once(' ').unwrap_or((&line.text, ""));
        if name != LABEL && !ROW_ORDER.contains(&name) {
            submissions.push(interface_line(name, rest, &label, &seen, line.number)?);
        } else if !submissions.is_empty() {
            return Err(fault(
                line.number,
                "a label or row field after the block's first interface",
            ));
        } else if name == LABEL {
            label = label_line(&label, rest, line.number)?;
        } else {
            row_field(&mut seen, name, rest, line.number)?;
        }
    }
    if submissions.is_empty() {
        let number = block.first().map_or(0, |line| line.number);
        return Err(fault(number, "a block with no interface line"));
    }
    Ok(submissions)
}

/// The label `rest` gives on the line numbered `number`, the block's first, where `label` is the one read so far.
fn label_line(label: &str, rest: &str, number: usize) -> Result<String, RegisterError> {
    if !label.is_empty() {
        return Err(fault(number, "a second label"));
    }
    unescape(rest, number)
}

/// Add the field `name`, of the row, with the value `rest` on the line numbered `number`, to `seen`.
fn row_field(seen: &mut BTreeMap<String, String>, name: &str, rest: &str, number: usize) -> Result<(), RegisterError> {
    if seen.insert(name.to_owned(), unescape(rest, number)?).is_some() {
        return Err(fault(number, "a row field given twice"));
    }
    Ok(())
}

/// The submission the interface line numbered `number` gives.
///
/// The line is the interface `name`, then `rest`, its values or the reason it was withheld; the submission is filed as
/// `label` and seen at `seen`.
fn interface_line(
    name: &str,
    rest: &str,
    label: &str,
    seen: &BTreeMap<String, String>,
    number: usize,
) -> Result<Submission, RegisterError> {
    if let Some(reason) = rest.strip_prefix(BLOCKED) {
        let reason = unescape(reason, number)?;
        return Ok(Submission::withheld(name, label, &reason, seen.clone()));
    }
    let values: Vec<&str> = rest.split(' ').collect();
    let names = result_names(name);
    if values.len() != names.len() {
        return Err(fault(number, "not as many values as the interface has results"));
    }
    let results = names
        .into_iter()
        .map(str::to_owned)
        .zip(values.into_iter().map(str::to_owned));
    Ok(Submission::signature(name, label, results.collect(), seen.clone()))
}

/// `text` with each of its escapes, `\\` and `\u{...}`, read as the character it stands for.
fn unescape(text: &str, number: usize) -> Result<String, RegisterError> {
    let mut read = String::with_capacity(text.len());
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            read.push(character);
            continue;
        }
        match characters.next() {
            Some('\\') => read.push('\\'),
            Some('u') => {
                let rest = characters.as_str();
                let hex = rest
                    .strip_prefix('{')
                    .and_then(|braced| braced.split_once('}'))
                    .ok_or_else(|| fault(number, "a \\u escape with no braced code point"))?;
                let code = u32::from_str_radix(hex.0, 16)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or_else(|| fault(number, "a \\u escape naming no character"))?;
                read.push(code);
                characters = hex.1.chars();
            }
            _ => return Err(fault(number, "a backslash that starts no escape")),
        }
    }
    Ok(read)
}

/// The fault `problem` on the line numbered `number`.
const fn fault(number: usize, problem: &'static str) -> RegisterError {
    RegisterError::Submitted { line: number, problem }
}

/// `submissions` laid out as a file of submissions, a block for each label and row in the order each is first given.
#[must_use]
pub fn write(submissions: &[Submission]) -> String {
    let mut blocks: Vec<Vec<&Submission>> = Vec::new();
    for submission in submissions {
        let same = |block: &&mut Vec<&Submission>| {
            block
                .first()
                .is_some_and(|first| first.label == submission.label && first.seen == submission.seen)
        };
        match blocks.iter_mut().find(same) {
            Some(block) => block.push(submission),
            None => blocks.push(vec![submission]),
        }
    }
    blocks
        .iter()
        .map(|block| block_text(block))
        .collect::<Vec<_>>()
        .join("\n")
}

/// One block of a file: the label and row its submissions share, then a line for each submission.
fn block_text(block: &[&Submission]) -> String {
    let mut text = String::default();
    if let Some(first) = block.first() {
        text.push_str(&line_of(LABEL, &escaped(&first.label)));
        let given = |field: &&str| first.seen.contains_key(*field);
        let others = first
            .seen
            .keys()
            .map(String::as_str)
            .filter(|field| !ROW_ORDER.contains(field));
        for field in ROW_ORDER.iter().copied().filter(given).chain(others) {
            let value = first.seen.get(field).map_or("", String::as_str);
            text.push_str(&line_of(field, &escaped(value)));
        }
    }
    for submission in block {
        let rest = if submission.blocked {
            [BLOCKED, &escaped(&submission.reason)].concat()
        } else {
            let values: Vec<&str> = submission.results.pairs().iter().map(|pair| pair.1.as_str()).collect();
            values.join(" ")
        };
        text.push_str(&line_of(&submission.interface, &rest));
    }
    text
}

/// `text` in ASCII alone: a backslash as `\\`, and each character past ASCII, and each control character, as `\u{...}`.
fn escaped(text: &str) -> String {
    text.chars()
        .map(|character| match character {
            '\\' => "\\\\".to_owned(),
            ' '..='~' => character.to_string(),
            other => format!("\\u{{{:x}}}", u32::from(other)),
        })
        .collect()
}

/// The line `name`, then `text` after a space, broken into lines no longer than [`LONGEST_LINE`].
///
/// Each line ends in a newline.
/// It is broken at a space where one fits, a single space with a character either side, which the next line's
/// [`CONTINUED`] stands for; and otherwise between two characters, never inside an escape and never just after a space,
/// the next line opening [`JOINED`].
fn line_of(name: &str, text: &str) -> String {
    let mut atoms = atoms(&[name, " ", text].concat());
    let mut lines = String::default();
    // The atoms before `first` open a continuation, and are never broken at.
    let mut first = 0_usize;
    while width(&atoms) > LONGEST_LINE {
        let fits = fitting(&atoms);
        let space = space_before(&atoms, first, fits);
        let (cut, opening, skipped) = if space > first {
            (space, CONTINUED, 1)
        } else {
            (cut_before(&atoms, first, fits), JOINED, 0)
        };
        let rest = atoms.split_off(cut);
        lines.push_str(&atoms.concat());
        lines.push('\n');
        atoms = std::iter::once(opening.to_owned())
            .chain(rest.into_iter().skip(skipped))
            .collect();
        first = 1;
    }
    lines.push_str(&atoms.concat());
    lines.push('\n');
    lines
}

/// `text` split into the pieces a line may be broken between: each escape whole, and each other character.
fn atoms(text: &str) -> Vec<String> {
    let mut atoms = Vec::new();
    let mut rest = text;
    while let Some(character) = rest.chars().next() {
        let length = if rest.starts_with("\\\\") {
            2
        } else if rest.starts_with("\\u{") {
            rest.find('}').map_or(1, |end| end.saturating_add(1))
        } else {
            character.len_utf8()
        };
        let (atom, tail) = rest.split_at(length);
        atoms.push(atom.to_owned());
        rest = tail;
    }
    atoms
}

/// How long the line `atoms` make is.
fn width(atoms: &[String]) -> usize {
    atoms.iter().map(String::len).sum()
}

/// How many of `atoms`, from the first, fit in [`LONGEST_LINE`].
fn fitting(atoms: &[String]) -> usize {
    let mut length = 0_usize;
    atoms
        .iter()
        .take_while(|atom| {
            length = length.saturating_add(atom.len());
            length <= LONGEST_LINE
        })
        .count()
}

/// Whether the atom at `index` of `atoms` is a space.
fn is_space(atoms: &[String], index: usize) -> bool {
    atoms.get(index).is_some_and(|atom| atom == " ")
}

/// The last single space among `atoms` after `first` and at or before `last`, or `first` where there is none.
///
/// A single space is one with a character either side.
fn space_before(atoms: &[String], first: usize, last: usize) -> usize {
    (first.saturating_add(1)..=last)
        .rev()
        .find(|&index| {
            let after = index.saturating_add(1);
            is_space(atoms, index)
                && !is_space(atoms, index.saturating_sub(1))
                && after < atoms.len()
                && !is_space(atoms, after)
        })
        .unwrap_or(first)
}

/// Where `atoms` too long for a line are cut between two characters.
///
/// That is after the last of the `fits` that fit, stepped back past any spaces so that no line ends in one, but never
/// to before the first atom after `first`.
fn cut_before(atoms: &[String], first: usize, fits: usize) -> usize {
    let mut cut = fits;
    while cut > first.saturating_add(1) && is_space(atoms, cut.saturating_sub(1)) {
        cut = cut.saturating_sub(1);
    }
    cut
}

/// The name a file holding `text` takes: the start of its SHA-256 in hex, then `.txt`.
///
/// The same submissions therefore always name the same file.
#[must_use]
pub fn name_of(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    let digit = |value: u8| char::from_digit(u32::from(value), 16).unwrap_or('0');
    let hex: String = digest
        .iter()
        .flat_map(|byte| [digit(byte >> 4_u32), digit(byte & 0x0f)])
        .collect();
    format!("{}.txt", hex.get(..NAME_DIGITS).unwrap_or(&hex))
}

/// What stopped a file of submissions being written.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProposeError {
    /// The environment could not be read.
    #[error("the environment could not be read: {0}")]
    Env(#[from] serde_env::Error),
    /// The file could not be written.
    #[error("the file of submissions could not be written: {0}")]
    Io(#[from] std::io::Error),
}

/// Write `submissions` as a file of submissions in the directory `dir`, made if it does not exist, giving its path.
pub fn propose(dir: &Path, submissions: &[Submission]) -> Result<PathBuf, ProposeError> {
    let text = write(submissions);
    fs_err::create_dir_all(dir)?;
    let path = dir.join(name_of(&text));
    fs_err::write(&path, text)?;
    Ok(path)
}

/// Write `submissions` with [`propose`] into the directory `GPU_PROBE_INTO` names, giving the paths written.
///
/// None is written where the variable names no directory. A measurement that runs in CI as well as by hand calls this,
/// so that a run setting the variable gathers what each platform answered as files to add to the registry.
pub fn propose_where_asked(submissions: &[Submission]) -> Result<Vec<PathBuf>, ProposeError> {
    let env = crate::config::load()?;
    if env.gpu.probe_into.is_empty() {
        return Ok(Vec::new());
    }
    Ok(vec![propose(Path::new(&env.gpu.probe_into), submissions)?])
}
